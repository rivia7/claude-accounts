import Foundation
import Observation

/// 侧栏选中的东西：办公室（首页，所有账号一眼看全）、某个账号、节点分流。
enum SidebarItem: Hashable, Sendable {
    case office
    case account(AccountRef)
    /// 用量统计：所有官方账号每周用了多少，放在一起比
    case usageStats
    case routing
    case clientUpdates

    var account: AccountRef? { if case .account(let r) = self { return r }; return nil }
}

enum ActivityKind: Sendable, Equatable {
    case app, web, billing, limitResets, check, sync, rename, reset
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
    /// 在 App 里问完的流程（提问会话，PROTOCOL 4.6）。
    case flow(FlowModel)

    var id: String {
        switch self {
        case .handoff(let h): return "handoff:" + h.id
        case .failure(let f): return "failure:" + f.id.uuidString
        case .flow(let f): return "flow:" + f.id.uuidString
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
    /// 用这个终端打开交接脚本；打不开时回调 false。
    var openInTerminal: @MainActor (URL, TerminalApp, @escaping @MainActor (Bool) -> Void) -> Void = { _, _, done in done(true) }
    /// 打开这个终端（半自动：命令在剪贴板里，用户自己粘贴）；打不开时回调 false。
    var openTerminalApp: @MainActor (TerminalApp, @escaping @MainActor (Bool) -> Void) -> Void = { _, done in done(true) }
    /// 后台干活之前看的系统状态（用户走开了没有、低电量模式、过热）。
    var systemConditions: @MainActor () -> SystemConditions = { SystemConditions() }
    /// 这台 Mac 上装了哪些终端（系统自带的在最前面）。
    var installedTerminals: @MainActor () -> [TerminalApp] = { [.system] }
    /// 选一个文件夹（从这里开始看）；取消时是 nil。
    var chooseDirectory: @MainActor (String?) async -> String? = { _ in nil }
    var openURL: @MainActor (URL) -> Void = { _ in }
    var copy: @MainActor (String) -> Void = { _ in }
    /// 复制密文（API key）：剪贴板标成机密，过一会儿还是它就清掉（App 里是 SecretClipboard）。不进提示、不进读屏。
    var copySecret: @MainActor (String) -> Void = { _ in }
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
    /// 自动刷新上次试过的时间：查失败时 fetchedEpoch 不变，靠它按间隔重试，不每分钟试一次。
    @ObservationIgnored private var usageAttempted: [AccountRef: Double] = [:]
    /// 自动检测出口：上次试的时间、正在测的工具
    @ObservationIgnored private var exitAttempted: [AccountRef: Double] = [:]
    @ObservationIgnored private var exitPolling: Set<ToolID> = []
    /// 什么时候点的“重新登录”（UsagePolling.reloginWatch 之内盯着它）
    @ObservationIgnored private var reloginStarted: [AccountRef: Double] = [:]
    /// 写回账号表的第几次（连着拖几次时只有最后一次收尾）；这次运行看过 App 里有没有没写回的顺序了没有
    @ObservationIgnored private var orderGeneration = 0
    @ObservationIgnored private var orderChecked = false
    /// 后台自动刷新用量时遇到的、需要用户同意的事（不弹框，显示一条提示，点了再手动刷新）。
    private(set) var usageNeedsConsent: [ToolID: [ConsentEvent]] = [:]
    /// 后台自动刷新用量失败（不弹框，显示一条可以重试的提示）。
    private(set) var usageFailure: [ToolID: Failure] = [:]
    private(set) var activities: [AccountRef: Activity] = [:]
    /// 账号在各组里的排序和置顶（用户拖的；存在 App 的设置里）
    private(set) var arrangement = AccountArrangement()
    private(set) var syncActivity: Activity?
    /// 正在拉维护者的映射（定时的和手动的同一时间一个）。
    private(set) var policyPulling = false
    /// 账号页「设备」一节（`cca [codex] devices KEY --json`，PROTOCOL 3.4）：账号页看得见时一分钟查一次，只记在内存里
    private(set) var devices: [AccountRef: DevicesSnapshot] = [:]
    private(set) var devicesLoading: Set<AccountRef> = []
    /// 「更新并重新打开」做到哪一步了：先把 cca 换成检查更新找到的新版本（`cca update 版本号`），再下载和它同版本的 App
    /// （`cca gui update`）；cca 已经比 App 新时只有后一步。
    enum UpdateStep: Equatable { case cca, app }
    private(set) var updateStep: UpdateStep?
    var updating: Bool { updateStep != nil }
    /// 最近一次检查更新的结果（`cca update --check --json`，PROTOCOL 5.1）
    private(set) var updateInfo: UpdateDoc?
    private(set) var updateChecking = false
    private(set) var clientUpdates: ClientUpdatesDoc?
    /// 有新版本的客户端有几个（侧栏「客户端更新」右边的标签）
    var clientUpdatesReady: Int { clientUpdates?.updatableCount ?? 0 }
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

    // MARK: 终端（记在 UserDefaults 里）

    /// 这台 Mac 上装了的终端，系统自带的在最前面。
    private(set) var terminals: [TerminalApp] = [.system]
    /// 用户挑的默认终端（bundle id）；没挑过是 nil。
    private(set) var terminalID: String?
    /// 账号的终端会话最近在哪些文件夹里打开过，最新的在前。所有账号共用：额度用完换个账号，接着在同一个项目里干活。
    private(set) var terminalDirectories: [String] = []

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
            reconcileDevicesPolling(restart: true)
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
    /// 看得见的账号页有「设备」一节（cca 说这个账号有：映射里的官方账号、配置了设备统计的服务器）：这个账号
    var devicesRef: AccountRef? {
        guard mainVisible, case .account(let r)? = selection, account(r)?.can.devices == true else { return nil }
        return r
    }

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
    /// 交接脚本放在哪（测试里换成临时目录）。
    @ObservationIgnored var handoffDirectory = TerminalHandoff.defaultDirectory()
    /// 等多久（秒）：选的终端过了这么久还没运行交接脚本，就请用户改用系统自带的。留够时间给 iTerm2 问“要运行吗”。
    @ObservationIgnored var terminalGrace: Double = 30

    enum Prefs {
        static let tool = "tool"
        /// 上次检查更新的时间（Unix 秒）：自动检查一天最多一次；查不了时记成 3 小时后再查
        static let updateCheckedAt = "update.checkedAt"
        /// 点过“以后再说”的版本
        static let updateDismissed = "update.dismissed"
        /// 默认终端（bundle id）
        static let terminalApp = "terminal.app"
        /// 账号的终端会话最近打开过的文件夹（一行一个）
        static let terminalDirectories = "terminal.directories"
        /// 账号在各组里的排序和置顶（AccountArrangement 的 JSON）
        static let arrangement = "accounts.arrangement"
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
        terminalID = defaults.string(forKey: Prefs.terminalApp)
        terminalDirectories = (defaults.string(forKey: Prefs.terminalDirectories) ?? "").split(separator: "\n").map(String.init)
        if let s = defaults.string(forKey: Prefs.arrangement),
           let saved = try? JSONDecoder().decode(AccountArrangement.self, from: Data(s.utf8)) {
            arrangement = saved
        }
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
        self.scheduler.onUsageTick = { [weak self] in self?.refreshUsageIfNeeded() }
        self.scheduler.clientCheckDelay = { [weak self] in self?.clientCheckDelay() ?? 3600 }
        self.scheduler.onLiveTick = { [weak self] in self?.refreshLive() }
        self.scheduler.onClientCheckTick = { [weak self] in self?.checkClientUpdates(manual: false) }
        self.scheduler.policyDelay = { [weak self] in self?.policyDelay() ?? PolicyPolling.fallback }
        self.scheduler.onPolicyTick = { [weak self] in self?.pollPolicy() }
        self.scheduler.onDevicesTick = { [weak self] in self?.pollDevices() }
    }

    // MARK: 启动和连接

    /// 启动：找 cca → `_gui hello` → `_gui state` → 看得见的话 `_gui live` 和用量。
    func start() async {
        guard !startedOnce else { return }
        startedOnce = true
        refreshTerminals()
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
        defer { reconcileBackgroundPolling() }
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
            reconcilePolicyPolling()   // 映射可能是刚配置的
            reconcileDevicesPolling()  // 映射里刚加了设备统计的服务器、这个账号刚进了映射
            // 2.9.0-beta.17 的顺序只记在 App 里：第一次读到账号表时写回去
            if !orderChecked {
                orderChecked = true
                if !arrangement.order.isEmpty { writeOrder() }
            }
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

    /// 登录已失效（查用量时才知道：本地的登录信息还在，所以 cca 算的 login 还是已登录）。
    func loginExpired(_ a: Account) -> Bool { usage(for: a)?.errorCode == .auth }

    /// 周结算提醒：本周额度快重置了、还剩不少，建议先用（侧栏、菜单栏的沙漏）。和账号页那句话、总览一样从 Diagnosis 取。
    func useFirst(for a: Account, now: Double) -> UseFirst? {
        for d in Diagnosis.all(a, usage: usage(for: a), live: liveSlot(for: a.ref), now: now) {
            if case .useFirst(let f) = d { return f }
        }
        return nil
    }

    /// 自动刷新用量、检测出口（定时器每分钟一次，App 变得看得见时也来一次）：只查到期的账号（UsagePolling、ExitPolling）。
    func refreshUsageIfNeeded() {
        guard phase == .ready else { return }
        let conditions = hooks.systemConditions()
        guard conditions.idleSeconds < UsagePolling.awayAfter, !conditions.thermalPressure else { return }
        guard dirty, !isVisible else {
            startDueRuns(lowPower: conditions.lowPower)
            return
        }
        dirty = false   // 看不见时 state 只记下“变了”，要判断哪些账号在用时才读
        track { [weak self] in
            await self?.loadState()
            self?.startDueRuns(lowPower: conditions.lowPower)
        }
    }

    private func startDueRuns(lowPower: Bool) {
        startDueUsageRuns(lowPower: lowPower)
        startDueExitChecks(lowPower: lowPower)
    }

    /// 刚点了“重新登录”、还没看到成功：这段时间每分钟查一次（登录过程中查到“没登录”不花什么）。
    /// Claude Code 记的套餐过时了的也算：重新登录之后要再查一次用量，cca 才知道它记下了现在的套餐，提示才会消失。
    private func isRelogging(_ a: Account, now: Double) -> Bool {
        guard let started = reloginStarted[a.ref], now - started < UsagePolling.reloginWatch else { return false }
        return a.login != .loggedIn || usage(for: a)?.errorCode != nil || usage(for: a)?.outdatedLoginPlan != nil
    }

    private func startDueUsageRuns(lowPower: Bool) {
        let now = Date().timeIntervalSince1970
        for t in visibleTools where toolState(t)?.enabled ?? (t == .claude) {
            // 同一个工具一次只查一批；看不见时，等着用户同意的不白跑
            guard !usageRefreshing.contains(t), !usageRefreshingKeys.contains(where: { $0.tool == t }),
                  isVisible || usageNeedsConsent[t] == nil else { continue }
            let items = accounts(t).filter { ($0.can.usage && $0.login == .loggedIn) || isRelogging($0, now: now) }.map { a in
                UsagePolling.Item(key: a.key, inUse: a.appRunning || a.cliSessions > 0,
                                  updated: max(usage(for: a)?.fetchedEpoch ?? 0, usageAttempted[a.ref] ?? 0),
                                  login: isRelogging(a, now: now) ? .relogging : loginExpired(a) ? .expired : .normal)
            }
            let keys = UsagePolling.dueKeys(items, visible: isVisible, lowPower: lowPower, now: now)
            if !keys.isEmpty { pollUsage(t, keys: keys, now: now) }
        }
    }

    private func pollUsage(_ t: ToolID, keys: [String], now: Double) {
        guard let client else { return }
        let refs = keys.map { AccountRef(tool: t, key: $0) }
        for r in refs { usageAttempted[r] = now }
        usageRefreshingKeys.formUnion(refs)
        track { [weak self] in
            let (doc, r) = await client.document(.usagePoll(t, keys: keys), as: UsageDoc.self, prompter: nil)
            guard let self else { return }
            self.usageRefreshingKeys.subtract(refs)
            await self.finishUsage(t, doc: doc, result: r, manual: false)
        }
    }

    /// 自动检测出口：只测到期的账号（ExitPolling），结果 cca 记进各账号的 health.json，读一次 state 就有了。不弹框、不占用账号的按钮；
    /// 没测成就等下一轮（`exitAttempted`），账号页上照样有“多久以前测的”。没登录的账号用不了，不测。
    private func startDueExitChecks(lowPower: Bool) {
        guard let client else { return }
        let now = Date().timeIntervalSince1970
        for t in visibleTools where toolState(t)?.enabled ?? (t == .claude) {
            // 一个工具一次只测一批；正在查用量时等下一分钟（两边都要核实路由器上的节点，错开）
            guard !exitPolling.contains(t), !usageRefreshing.contains(t),
                  !usageRefreshingKeys.contains(where: { $0.tool == t }) else { continue }
            let items = accounts(t).filter { $0.can.check && ($0.isAPI || $0.login == .loggedIn) && activities[$0.ref] == nil }.map { a in
                // 换了节点的旧结果说的不是现在的出口：算没测过
                UsagePolling.Item(key: a.key, inUse: a.appRunning || a.cliSessions > 0,
                                  updated: max(a.health?.applies == true ? a.health?.checkedAt ?? 0 : 0, exitAttempted[a.ref] ?? 0))
            }
            let keys = ExitPolling.dueKeys(items, visible: isVisible, lowPower: lowPower, now: now)
            guard !keys.isEmpty else { continue }
            for k in keys { exitAttempted[AccountRef(tool: t, key: k)] = now }
            exitPolling.insert(t)
            track { [weak self] in
                _ = await client.execute(.checkPoll(t, keys: keys), prompter: nil)
                guard let self else { return }
                self.exitPolling.remove(t)
                await self.loadState()
            }
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
                await self.finishUsage(t, doc: doc, result: r, manual: manual)
            }
        }
    }

    private func finishUsage(_ t: ToolID, doc: UsageDoc?, result r: ExecResult, manual: Bool) async {
        if let doc {
            var entries = usageRuns[t] ?? [:]
            for e in doc.accounts { if let k = e.key ?? e.account { entries[k] = e } }
            usageRuns[t] = entries
        }
        switch r.status {
        case .success:
            usageNeedsConsent[t] = nil
            usageFailure[t] = nil
        case .needsConsent(let cs):
            usageNeedsConsent[t] = cs
        case .refused:
            showToast("没有同意，没有做任何改动", .info)
        case .terminal(let e):
            if manual { presentHandoff(e) }
        case .failed(let f):
            if f.reason == .cancelled { break }
            if manual {
                usageFailure[t] = nil   // 已经弹框说过了，不再留着后台那一条
                present(.failure(FailureInfo(title: "没能刷新用量", failure: f)))
            } else {
                usageFailure[t] = f
            }
        }
        await loadState()
    }

    // MARK: 动作

    func openApp(_ ref: AccountRef) {
        launch(ref, kind: .app, command: .app(ref.tool, key: ref.key), title: "没能打开 \(appLabel(ref.tool))")
    }

    func openWeb(_ ref: AccountRef) {
        launch(ref, kind: .web, command: .web(ref.tool, key: ref.key), title: "没能打开网页版")
    }

    /// 查看 API 额度：在账号自己的浏览器里打开控制台的账单页（网址是 cca 的事）。
    func openBilling(_ ref: AccountRef) {
        launch(ref, kind: .billing, command: .billing(ref.tool, key: ref.key), title: "没能打开账单页")
    }

    /// 查看重置次数：在账号自己的浏览器里打开 claude.ai 的用量页（Claude 账号的次数和到期日只在那里显示）。
    func openLimitResets(_ ref: AccountRef) {
        launch(ref, kind: .limitResets, command: .limitResets(ref.tool, key: ref.key), title: "没能打开用量页")
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
            let opened = kind == .web ? "已打开网页版" : kind == .billing ? "已打开账单页" : kind == .limitResets ? "已打开用量页" : "已打开 \(self.appLabel(ref.tool))"
            self.finish(r, failureTitle: title, success: r.resultEvent?.message ?? opened)
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
            presentHandoff(e)
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
                self.showToast(newKey == ref.key ? "别名没有变" : "别名改好了：登录、资料和端口都不变", .success)
            case .failed(let f) where f.reason == .command:
                self.activities[ref] = nil
                self.renameError = nonEmpty(f.message) ?? "没能改别名"
            default:
                self.activities[ref] = nil
                self.aliasEditing = nil
                self.finish(r, failureTitle: "没能改别名", success: "别名改好了")
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
        // 流程问到一半时不能被别的弹层顶掉：排着，流程结束再出
        if flow != nil {
            queuedSheet = s
            return
        }
        sheet = s
        hooks.showMainWindow()
    }

    // MARK: 提问会话（PROTOCOL 4.6）

    /// 正在 App 里问的流程（同一时间一个）。
    private(set) var flow: FlowModel?
    @ObservationIgnored private var queuedSheet: Sheet?

    var promptSupported: Bool { hello?.prompt == PromptAPI.version }
    var formsSupported: Bool { promptSupported && hello?.forms == PromptAPI.forms }

    private func canAskInApp(_ h: Handoff) -> Bool {
        h.inApp && promptSupported && (!h.needsForms || formsSupported)
    }

    /// 运行一条要和用户交互的命令：能在 App 里问完的在 App 里问，否则（cca 太旧、这条还没搬过来）开终端。
    func run(_ h: Handoff) {
        guard canAskInApp(h), flow == nil, let client else {
            if flow == nil { runInTerminal(h) }
            return
        }
        do {
            let session = try client.startSession(h.arguments)
            let f = FlowModel(handoff: h, session: session) { [weak self] flow, end in
                self?.flowEnded(flow, end)
            }
            flow = f
            sheet = .flow(f)
            hooks.showMainWindow()
        } catch {
            runInTerminal(h)
        }
    }

    private func flowEnded(_ f: FlowModel, _ end: FlowModel.End) {
        guard flow === f else { return }
        var result: ResultEvent?
        switch end {
        case .success(let r):
            result = r
            if let m = nonEmpty(r?.message) { showToast(m, .success) }
        case .cancelled:
            break
        case .terminal(let t):
            queuedSheet = .handoff(Handoff(event: t))
        }
        finishFlow(result)
    }

    /// 关掉结束了的流程（失败页、提醒页的“好”）。
    func closeFlow(_ f: FlowModel) {
        guard flow === f else { return }
        finishFlow(f.result)
    }

    private func finishFlow(_ result: ResultEvent?) {
        flow = nil
        sheet = queuedSheet
        queuedSheet = nil
        if sheet != nil { hooks.showMainWindow() }
        Task { [weak self] in
            guard let self else { return }
            // 重新跑 hello：第一次添加 ChatGPT 账号时 Codex 才启用
            await self.connect()
            if let r = result, r.code == .added, let key = r.key {
                let ref = AccountRef(tool: r.tool ?? .claude, key: key)
                if self.account(ref) != nil { self.show(ref) }
            }
            if self.liveVisible { self.refreshLive() }
        }
    }

    func dismissSheet() {
        sheet = nil
    }

    /// cca 说这一步要用户来做（`terminal` 事件）：配置路由器、配置本机 Clash 能在 App 里问，别的交给终端。
    private func presentHandoff(_ e: TerminalEvent) {
        let form: Handoff? = e.code == .routerSetup ? .routerSetup : e.code == .localSetup ? .localSetup : nil
        if let form, canAskInApp(form), flow == nil {
            run(form)
        } else {
            present(.handoff(Handoff(event: e)))
        }
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

    /// 账号的会话用哪个终端（别的命令固定用系统自带的，见 `Handoff.systemTerminal`）：用户选的；没选过（或者选的卸载了）时用第一个装了的全自动第三方终端，都没有就是系统自带的。半自动的要用户自己选。
    var terminalChoice: TerminalApp {
        terminals.first { $0.id == terminalID } ?? terminals.first { $0 != .system && $0.isAutomatic } ?? .system
    }

    func isInstalled(_ terminal: TerminalApp) -> Bool {
        terminals.contains(terminal)
    }

    /// 选默认的终端。没装的选不了（选择列表里是灰的）。
    func chooseTerminal(_ id: String) {
        guard terminals.contains(where: { $0.id == id }) else { return }
        terminalID = id
        defaults.set(id, forKey: Prefs.terminalApp)
    }

    func refreshTerminals() {
        terminals = hooks.installedTerminals()
    }

    /// 终端会话可选的文件夹：最近用过、现在还在的，最新的在前。
    var sessionDirectories: [String] {
        terminalDirectories.filter { path in
            var isDirectory: ObjCBool = false
            return FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory) && isDirectory.boolValue
        }
    }

    /// 给人看的路径（主目录写成 ~）。
    func displayPath(_ path: String) -> String {
        if path == homeDir { return "~" }
        return path.hasPrefix(homeDir + "/") ? "~" + path.dropFirst(homeDir.count) : path
    }

    private func rememberDirectory(_ path: String) {
        guard !path.contains("\n") else { return }
        terminalDirectories = Array(([path] + terminalDirectories.filter { $0 != path }).prefix(8))
        defaults.set(terminalDirectories.joined(separator: "\n"), forKey: Prefs.terminalDirectories)
    }

    /// 在终端里用这个账号：不给文件夹就用最近用的那个，一个都没有时先选一个。
    func openSession(_ ref: AccountRef, directory: String? = nil) {
        guard let a = account(ref), a.can.cli else { return }
        guard let dir = directory ?? sessionDirectories.first ?? (isFixtureMode ? homeDir : nil) else {
            chooseSessionDirectory(ref)
            return
        }
        rememberDirectory(dir)
        runInTerminal(.session(prefix: prefix(a.tool), key: a.key, name: a.displayName, directory: dir))
    }

    /// 选一个文件夹，在里面打开这个账号的终端会话（选了就记住）。
    func chooseSessionDirectory(_ ref: AccountRef) {
        Task { [weak self] in
            guard let self, let dir = await self.hooks.chooseDirectory(self.sessionDirectories.first ?? self.homeDir) else { return }
            self.openSession(ref, directory: dir)
        }
    }

    /// 在选好的终端里运行一条 cca 命令（PROTOCOL 6）。
    func runInTerminal(_ h: Handoff) {
        if isFixtureMode {
            showToast("演示模式：不会打开终端（\(h.display)）", .info)
            return
        }
        guard let path = ccaPath else {
            showToast("找不到 cca", .warning)
            return
        }
        launchInTerminal(h, ccaPath: path)
    }

    func launchInTerminal(_ h: Handoff, ccaPath path: String) {
        let app = h.systemTerminal ? TerminalApp.system : terminalChoice
        guard app.isAutomatic else {
            pasteInTerminal(h, app, ccaPath: path)
            return
        }
        let url: URL
        do {
            url = try TerminalHandoff.write(h, ccaPath: path, directory: handoffDirectory)
        } catch {
            showToast("没能写交接文件：\(error.localizedDescription)", .warning)
            return
        }
        hooks.openInTerminal(url, app) { [weak self] opened in
            if !opened { self?.terminalDidNotRun(url, h, app) }
        }
        guard app != .system else { return }
        let grace = terminalGrace
        Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(grace * 1_000_000_000))
            self?.terminalDidNotRun(url, h, app)
        }
    }

    /// 半自动：打开终端，把命令放进剪贴板，用户在它的终端里粘贴运行（不写交接文件）。
    private func pasteInTerminal(_ h: Handoff, _ app: TerminalApp, ccaPath path: String) {
        hooks.copy(h.pasteCommand(ccaPath: path))
        hooks.openTerminalApp(app) { [weak self] opened in
            if opened {
                self?.showToast("命令已复制：到 \(app.name) 的本地终端里粘贴运行", .info)
            } else {
                self?.showToast("打不开 \(app.name)，命令已复制，可以粘贴到别的终端里运行", .warning)
            }
        }
    }

    /// 选的终端没有运行交接脚本：收回脚本（它不会再被运行），问用户要不要改用系统自带的。脚本已经开始运行时什么也不做。
    private func terminalDidNotRun(_ url: URL, _ h: Handoff, _ app: TerminalApp) {
        guard TerminalHandoff.reclaim(url) else { return }
        var fallback = h
        fallback.systemTerminal = true
        fallback.title = "\(app.name) 没有运行这条命令"
        fallback.message = "可能它在等你确认，或者这个版本不支持被别的 App 打开。可以改用系统自带的「终端」，也可以复制命令自己运行（默认终端在「设置」里换）。"
        present(.handoff(fallback))
    }

    func copy(_ text: String) {
        hooks.copy(text)
        showToast("已复制：\(text)", .info)
    }

    /// 请求维护者把这个账号现在的节点锁进映射（PROTOCOL 4.8）：cca 写好一条维护者直接运行的命令，复制给用户发给维护者。
    func requestPolicyLock(_ a: Account) {
        if isFixtureMode {
            showToast("演示模式：不会生成请求", .info)
            return
        }
        guard let client else { return }
        Task { [weak self] in
            let r = await client.execute(.policyRequest(a.tool, key: a.key), prompter: nil)
            guard let self else { return }
            let line = String(decoding: r.last?.stdout ?? Data(), as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
            guard r.isSuccess, !line.isEmpty else {
                self.finish(r, failureTitle: "没能生成锁定请求", success: "cca 没有给出请求的内容")
                return
            }
            self.hooks.copy(line)
            self.showToast("已复制命令：发给维护者，在 cca 的仓库里运行后这个账号的节点就固定了", .success)
        }
    }

    // MARK: API key（PROTOCOL 4.10）

    /// 读这个账号存的 API key（`cca [codex] key KEY --show`，经内存里的管道）；读不了时弹框说原因、返回 nil。
    /// 明文不进被观察的状态、不进提示和读屏：调用的一方（复制、点开看）用完就丢。
    func readAPIKey(_ a: Account) async -> APIKeyValue? {
        guard phase == .ready, let client else { return nil }
        let r = await client.execute(.keyShow(a.tool, key: a.key), prompter: nil)
        if r.isSuccess, let v = APIKeyValue(cca: r.last?.stdout ?? Data()) { return v }
        if r.isSuccess {
            present(.failure(FailureInfo(title: "没能读取 API key", failure: Failure(reason: .command, code: nil, message: "cca 给的不是一个 key",
                                                                                 stderrTail: "", exitCode: 0, tool: a.tool, key: a.key))))
        } else {
            finish(r, failureTitle: "没能读取 API key", success: "")
        }
        return nil
    }

    /// 复制 API key：剪贴板标成机密（剪贴板管理器不记），60 秒后还是它就清掉。提示里不写 key。
    func copyAPIKey(_ a: Account) {
        Task { [weak self] in
            guard let self, let v = await self.readAPIKey(a) else { return }
            self.hooks.copySecret(v.text)
            self.showToast("已复制 \(a.displayName) 的 API key（60 秒后从剪贴板清掉）", .success)
        }
    }

    /// 记下 / 更换 API key：App 里的提问会话（密文框），cca 存进钥匙串。官方账号的只是备查，cca 不拿它启动。
    func recordAPIKey(_ a: Account) {
        run(.apiKey(prefix: prefix(a.tool), key: a.key, name: a.displayName, record: !a.isAPI))
    }

    /// 删掉官方账号记下的 API key（cca 先问：key_forget，默认不删）。
    func forgetAPIKey(_ a: Account) {
        guard phase == .ready, let client else { return }
        let prompter = hooks.prompter
        Task { [weak self] in
            let r = await client.execute(.keyForget(a.tool, key: a.key), prompter: prompter)
            guard let self else { return }
            self.finish(r, failureTitle: "没能删除 API key", success: r.resultEvent?.message ?? "已删除记下的 API key")
            await self.afterAction()
        }
    }

    func apply(_ remedy: Remedy) {
        switch remedy {
        case .none: break
        case .routerSync: syncRouter()
        case .handoff(let h): run(h)
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
        a.login == .loggedOut ? .login(prefix: prefix(a.tool), key: a.key, name: a.displayName)
            : .relogin(prefix: prefix(a.tool), key: a.key, name: a.displayName)
    }

    /// 登录、重新登录（在终端里）。`cca login` 会先退出现在的登录：登录还好好的账号先弹窗说清楚，点错了可以取消。
    func relogin(_ a: Account) {
        reloginStarted[a.ref] = Date().timeIntervalSince1970
        let h = loginHandoff(a)
        if a.login == .loggedIn, !loginExpired(a) {
            present(.handoff(h))
        } else {
            runInTerminal(h)
        }
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

    // MARK: 桌面客户端更新

    /// 连上 cca 之后才开始定时。App 隐藏时也检查客户端更新；检查绝不安装，开发和演示模式不自动运行。
    private func reconcileBackgroundPolling() {
        scheduler.updateUsagePolling(enabled: phase == .ready)
        scheduler.updateClientChecks(enabled: phase == .ready && !isFixtureMode && hello?.devMode != true)
        reconcilePolicyPolling()
    }

    private func reconcilePolicyPolling() {
        scheduler.updatePolicyPolling(enabled: phase == .ready && !isFixtureMode && policyPullAvailable)
    }

    // MARK: 账号连着的设备（PROTOCOL 3.4）

    /// 账号页的「设备」看得见时才定时查（换了账号马上查一次）；看不见、cca 说没有就停
    private func reconcileDevicesPolling(restart: Bool = false) {
        scheduler.updateDevicesPolling(enabled: phase == .ready && devicesRef != nil, restart: restart)
    }

    private func pollDevices() {
        guard let r = devicesRef, hooks.systemConditions().idleSeconds < UsagePolling.awayAfter else { return }   // 走开了：没人看
        refreshDevices(r, manual: false)
    }

    /// 问一次这个账号连着哪些设备。查不成时上次查到的照样显示，旁边说原因
    func refreshDevices(_ ref: AccountRef, manual: Bool) {
        guard phase == .ready, let client, !devicesLoading.contains(ref), account(ref)?.can.devices == true else { return }
        devicesLoading.insert(ref)
        track { [weak self] in
            let (doc, r) = await client.document(.devices(ref.tool, key: ref.key, auto: !manual), as: DevicesDoc.self, prompter: nil)
            guard let self else { return }
            self.devicesLoading.remove(ref)
            if let doc, let a = doc.account(ref.key) {
                self.devices[ref] = DevicesSnapshot(doc: doc, account: a, error: nil,
                                                    fetchedEpoch: doc.generatedEpoch ?? Date().timeIntervalSince1970)
            } else if r.failure?.reason != .cancelled {
                var s = self.devices[ref] ?? DevicesSnapshot()
                s.error = nonEmpty(r.failure?.message) ?? "没能查到这个账号连着的设备"
                self.devices[ref] = s
            }
        }
    }

    /// 能拉维护者的映射：配置了映射，cca 也够新（和 `hello.forms` 同一版起才有 `policy pull --json` 和 `next_check_epoch`；没有下次的时间，定时拉取就成了每 5 分钟一次）。
    var policyPullAvailable: Bool {
        formsSupported && state?.config?.policy?.configured == true
    }

    /// 离下次该拉维护者的映射还有多久：cca 给的时间（一小时一次）。到期了、没拉成时过一会儿再来。
    private func policyDelay() -> TimeInterval {
        PolicyPolling.delay(next: state?.config?.policy?.nextCheckEpoch, now: Date().timeIntervalSince1970)
    }

    /// 定时拉维护者的映射（`cca policy pull --json`）：有变化时 cca 照映射改账号表、同步路由器（要重载的不问，留着“待同步”，用户点同步时再问）。
    /// 在后台也拉（App 平时就在后台）；拉不成不打扰，到点再试，原因记在 cca 的日志里（`cca policy` 看得到）。
    private func pollPolicy() {
        guard phase == .ready, let client, !isFixtureMode, !policyPulling, flow == nil, policyPullAvailable,
              PolicyPolling.isDue(next: state?.config?.policy?.nextCheckEpoch, now: Date().timeIntervalSince1970),
              !hooks.systemConditions().thermalPressure else { return }
        policyPulling = true
        Task { [weak self] in
            let (doc, _) = await client.document(.policyPull(manual: false), as: PolicyPullDoc.self, prompter: nil)
            guard let self else { return }
            self.policyPulling = false
            if doc?.changed == true { self.showToast("维护者更新了节点映射，账号表已经跟着改好", .info) }
            await self.loadState()
        }
    }

    /// 现在就拉一次维护者的映射（节点分流页、账号的菜单）。结果照实提示；路由器要重载时接着走“同步路由器”：这次是用户点的，可以弹框问。
    func pullPolicyNow() {
        if isFixtureMode {
            showToast("演示模式：不会拉取映射", .info)
            return
        }
        guard phase == .ready, let client, !policyPulling, flow == nil, policyPullAvailable else { return }
        policyPulling = true
        Task { [weak self] in
            let (doc, r) = await client.document(.policyPull(manual: true), as: PolicyPullDoc.self, prompter: nil)
            guard let self else { return }
            self.policyPulling = false
            await self.loadState()
            let detail = nonEmpty(doc?.summary).map { "（\($0)）" } ?? ""
            self.finish(r, failureTitle: "没能拉取映射", success: (doc?.changed == true ? "已更新映射" : "映射没有变化") + detail)
            if r.isSuccess, r.last?.events.contains(where: { $0.consent != nil }) == true { self.syncRouter() }
        }
    }

    /// 离下次该检查客户端更新还有多久：cca 给的时间（间隔由 cca 决定）。已经到了或者没查成时过一会儿再来，不空转。
    private func clientCheckDelay() -> TimeInterval {
        guard let next = clientUpdates?.nextCheckEpoch else { return scheduler.clientCheckInterval }
        return max(next - Date().timeIntervalSince1970, min(scheduler.clientCheckInterval, 15 * 60))
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
            let conditions = hooks.systemConditions()   // 发现新版要下载、解包几百 MB：低电量、过热时晚点再说
            guard !conditions.lowPower, !conditions.thermalPressure else { return }
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
            showToast("要安装的版本变了，请重新检查后再安装", .warning)
            return
        }
        runClientUpdate(.clientInstall(token: token), activity: .install, manual: true, entryID: entry.id)
    }

    private func runClientUpdate(_ command: CcaCommand, activity: ClientUpdateActivity, manual: Bool, entryID: String? = nil) {
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
                    message = doc?.clients.first(where: { $0.id == entryID })?.detail ?? "客户端更新操作已完成"
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
        reconcileDevicesPolling()
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
        refreshTerminals()
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

    /// 添加这个组的账号（办公室各组的标题、工具栏的“＋”菜单）。
    func addHandoff(_ d: Employee.Department) -> Handoff { d.addHandoff(codexPrefix: prefix(.codex)) }

    /// 打开一个账号的页面（办公室的工位、分流图里的账号）：侧栏的工具跟着换过去。
    func show(_ ref: AccountRef) {
        if tool != ref.tool { tool = ref.tool }
        selection = .account(ref)
    }

    /// 办公室：按组排好的工位，每个账号此刻的样子（和账号页那句话用同一份用量、实况）。
    func office(now: Double) -> Office {
        Office.make(tools: state?.tools ?? [], codexEnabled: codexEnabled, usage: { self.usage(for: $0) },
                    live: { self.liveSlot(for: $0) }, now: now,
                    arrange: { d, people in self.arrangement.arranged(people, group: d.key, id: \.account.stableID) })
    }

    // MARK: 账号怎么排（拖动、置顶）

    func isOnTop(_ a: Account) -> Bool { arrangement.top.contains(a.stableID) }

    func setOnTop(_ a: Account, _ on: Bool) {
        arrangement.setTop(a.stableID, on)
        saveArrangement()
        writeOrder()
    }

    /// 这个组的全部账号，按用户排的顺序。第三方 API 账号的组是两个工具共用的。
    private func arranged(_ d: Employee.Department) -> [Account] {
        let members: [Account]
        switch d {
        case .claude: members = accounts(.claude).filter { !$0.isAPI }
        case .chatgpt: members = accounts(.codex).filter { !$0.isAPI }
        case .contractors: members = visibleTools.flatMap { accounts($0) }.filter(\.isAPI)
        }
        return arrangement.arranged(members, group: d.key, id: \.stableID)
    }

    /// 侧栏的分组：这个工具的官方账号（没有账号时这一组也在：标题上有添加按钮），有第三方 API 账号时再加一组。
    func accountGroups(_ t: ToolID) -> [AccountGroup] {
        let official: Employee.Department = t == .codex ? .chatgpt : .claude
        var out = [AccountGroup(department: official, accounts: arranged(official))]
        let api = arranged(.contractors).filter { $0.tool == t }
        if !api.isEmpty { out.append(AccountGroup(department: .contractors, accounts: api)) }
        return out
    }

    /// 用量统计页的分组：两个工具的官方账号（Claude 一直有，ChatGPT 在启用了 Codex 时有），和总览一样按用户排的顺序。
    /// 第三方 API 账号没有订阅的额度，不在里面。
    func officialGroups() -> [AccountGroup] {
        ([.claude] + (codexEnabled ? [.chatgpt] : [])).map { AccountGroup(department: $0, accounts: arranged($0)) }
    }

    /// 这个工具的账号（官方的在前，各组里按用户排的顺序）：菜单栏面板、第一次选中哪个账号用。
    func arrangedAccounts(_ t: ToolID) -> [Account] { accountGroups(t).flatMap(\.accounts) }

    /// 拖动排序：shown 是这一组现在显示的账号（置顶的在前面）。置顶的只在置顶的里面挪，没置顶的只在没置顶的里面挪，
    /// 拖过界的停在边上；要换区用「置顶」「取消置顶」。
    func moveAccounts(_ d: Employee.Department, shown: [Account], from offsets: IndexSet, to destination: Int) {
        guard let first = offsets.first, shown.indices.contains(first) else { return }
        let onTop = isOnTop(shown[first])
        let zone = shown.filter { isOnTop($0) == onTop }
        let start = onTop ? 0 : shown.count - zone.count
        let from = IndexSet(offsets.map { $0 - start }.filter { zone.indices.contains($0) })
        arrangement.move(group: d.key, full: arranged(d).map(\.stableID), shown: zone.map(\.stableID), from: from,
                         to: min(max(destination - start, 0), zone.count))
        saveArrangement()
        writeOrder()
    }

    /// 总览：把账号拖到另一张卡片上，排到那张卡片的位置。只认同一组、同一区（置顶的 / 没置顶的）里的。
    @discardableResult
    func dropAccount(_ id: String, onto target: Account) -> Bool {
        let d = Employee.department(of: target)
        let list = arranged(d)
        guard let from = list.firstIndex(where: { $0.stableID == id }), let to = list.firstIndex(of: target), from != to,
              isOnTop(list[from]) == isOnTop(target) else { return false }
        moveAccounts(d, shown: list, from: IndexSet(integer: from), to: from < to ? to + 1 : to)
        return true
    }

    private func saveArrangement() {
        guard !isFixtureMode, let d = try? JSONEncoder().encode(arrangement) else { return }
        defaults.set(String(decoding: d, as: UTF8.self), forKey: Prefs.arrangement)
    }

    /// 把现在的顺序写回账号表（终端里的菜单、编号也是这个顺序）。写之前界面先按拖出来的顺序显示，不等 cca；
    /// 写好、读回 state 后账号表就是唯一的依据（在终端里改了顺序，App 也跟着变）。连着拖几次时只有最后一次收尾。
    private func writeOrder() {
        guard let client, phase == .ready, !isFixtureMode, hello?.reorder == 1 else { return }
        orderGeneration += 1
        let generation = orderGeneration
        let plans = visibleTools.compactMap { t -> (ToolID, [String])? in
            let keys = arrangedAccounts(t).map(\.key)
            return keys == accounts(t).map(\.key) ? nil : (t, keys)
        }
        Task { [weak self] in
            var problem: String?
            for (t, keys) in plans {
                let r = await client.execute(.reorder(t, keys: keys), prompter: nil)
                if case .failed(let f) = r.status, f.reason != .cancelled { problem = f.message }
            }
            guard let self, generation == self.orderGeneration else { return }
            if !plans.isEmpty { await self.loadState() }
            guard generation == self.orderGeneration else { return }
            self.arrangement.order = [:]
            self.saveArrangement()
            if let problem { self.showToast("没能把顺序写进账号表：\(problem)", .warning) }
        }
    }

    private func defaultSelection(for t: ToolID) -> SidebarItem? {
        let list = arrangedAccounts(t)
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
        case .office?, .usageStats?, .routing?, .clientUpdates?:
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
