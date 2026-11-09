import Foundation

// PROTOCOL.md 里所有“枚举”字段：按字符串收，不认识的值不报错（isKnown 为 false，界面当“其它”处理）。

protocol KnownValues: OpenEnum {
    static var allKnown: [Self] { get }
}

extension KnownValues {
    var isKnown: Bool { Self.allKnown.contains(self) }
}

/// 工具：Claude（`cca …`）和 Codex（`cca codex …`，界面上叫 ChatGPT）。
struct ToolID: KnownValues, Comparable {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let claude = ToolID(rawValue: "claude")
    static let codex = ToolID(rawValue: "codex")
    static let allKnown: [ToolID] = [.claude, .codex]

    /// `cca` 后面的命令前缀（`cli_prefix`）：Claude 是空的，其它工具是 [工具名]（Codex 是 ["codex"]）。
    /// 这是唯一写死前缀的地方；`_gui state` 给了 `cli_prefix` 时界面以它为准。
    var defaultPrefix: [String] { self == .claude ? [] : [rawValue] }

    static func < (a: ToolID, b: ToolID) -> Bool {
        let ia = allKnown.firstIndex(of: a) ?? Int.max, ib = allKnown.firstIndex(of: b) ?? Int.max
        return ia != ib ? ia < ib : a.rawValue < b.rawValue
    }
}

struct AccountKind: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let oauth = AccountKind(rawValue: "oauth")
    static let api = AccountKind(rawValue: "api")
    static let allKnown: [AccountKind] = [.oauth, .api]
}

struct LoginState: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let loggedIn = LoginState(rawValue: "logged_in")
    static let loggedOut = LoginState(rawValue: "logged_out")
    static let emailMismatch = LoginState(rawValue: "email_mismatch")
    static let api = LoginState(rawValue: "api")
    static let allKnown: [LoginState] = [.loggedIn, .loggedOut, .emailMismatch, .api]
}

/// 套餐档位（`plan_tier`）：cca 按结构化的账号信息算好，App 不从套餐名猜（官方随时改叫法）。
/// `""` = 不知道（没登录、登录的不是这个账号、第三方 API 账号、认不出的新套餐、旧版 cca）。
struct PlanTier: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    /// Free、Go
    static let entry = PlanTier(rawValue: "entry")
    /// Pro、Plus、Team、Business、Enterprise…
    static let standard = PlanTier(rawValue: "standard")
    /// Max、Pro 100 / 200、Team Premium…
    static let advanced = PlanTier(rawValue: "advanced")
    /// Ultra、Pro 500
    static let premium = PlanTier(rawValue: "premium")
    static let unknown = PlanTier(rawValue: "")
    static let allKnown: [PlanTier] = [.entry, .standard, .advanced, .premium, .unknown]
}

/// `can_why` 的原因码。
struct CanWhy: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let apiAccount = CanWhy(rawValue: "api_account")
    static let appMissing = CanWhy(rawValue: "app_missing")
    static let noChrome = CanWhy(rawValue: "no_chrome")
    static let notLoggedIn = CanWhy(rawValue: "not_logged_in")
    static let toolMissing = CanWhy(rawValue: "tool_missing")
    static let allKnown: [CanWhy] = [.apiAccount, .appMissing, .noChrome, .notLoggedIn, .toolMissing]
}

/// 顶层 `errors[]`：config_unreadable / table_unreadable / account_unreadable（某个账号的登录状态文件读不了，
/// 这个账号当作没登录）；`tools[].errors[]`：table_invalid。
struct StateErrorCode: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let configUnreadable = StateErrorCode(rawValue: "config_unreadable")
    static let tableUnreadable = StateErrorCode(rawValue: "table_unreadable")
    static let accountUnreadable = StateErrorCode(rawValue: "account_unreadable")
    static let tableInvalid = StateErrorCode(rawValue: "table_invalid")
    static let allKnown: [StateErrorCode] = [.configUnreadable, .tableUnreadable, .accountUnreadable, .tableInvalid]
}

struct NoticeCode: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let policyAdded = NoticeCode(rawValue: "policy_added")
    static let policyAlias = NoticeCode(rawValue: "policy_alias")
    static let policyPinned = NoticeCode(rawValue: "policy_pinned")
    static let nodesFilled = NoticeCode(rawValue: "nodes_filled")
    static let allKnown: [NoticeCode] = [.policyAdded, .policyAlias, .policyPinned, .nodesFilled]
}

struct RoutePref: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let auto = RoutePref(rawValue: "auto")
    static let router = RoutePref(rawValue: "router")
    static let local = RoutePref(rawValue: "local")
    static let allKnown: [RoutePref] = [.auto, .router, .local]
}

/// 走哪边：`router` | `local`；live 的 `route` 还有 `none`（哪边都没配置）；health / result 的 `route` 还有
/// `direct`（直连的第三方 API 账号，不经端口）。
struct Route: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let router = Route(rawValue: "router")
    static let local = Route(rawValue: "local")
    static let none = Route(rawValue: "none")
    static let direct = Route(rawValue: "direct")
    static let allKnown: [Route] = [.router, .local, .none, .direct]
}

/// `routing.sync` / `routing.local_sync`。
struct SyncState: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let ok = SyncState(rawValue: "ok")
    static let pending = SyncState(rawValue: "pending")
    static let notApplicable = SyncState(rawValue: "n/a")
    static let allKnown: [SyncState] = [.ok, .pending, .notApplicable]
}

struct LocalClient: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let clashVerge = LocalClient(rawValue: "clash-verge")
    static let manual = LocalClient(rawValue: "manual")
    /// 内置代理：App 托管的 mihomo 内核（PROTOCOL 7）
    static let builtin = LocalClient(rawValue: "builtin")
    static let unset = LocalClient(rawValue: "")
    static let allKnown: [LocalClient] = [.clashVerge, .manual, .builtin, .unset]
}

struct Backend: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let openClash = Backend(rawValue: "OpenClash")
    static let shellCrash = Backend(rawValue: "ShellCrash")
    static let allKnown: [Backend] = [.openClash, .shellCrash]
}

struct LiveErrorCode: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let unreachable = LiveErrorCode(rawValue: "unreachable")
    static let statusFailed = LiveErrorCode(rawValue: "status_failed")
    static let notConfigured = LiveErrorCode(rawValue: "not_configured")
    static let notRunning = LiveErrorCode(rawValue: "not_running")
    static let allKnown: [LiveErrorCode] = [.unreachable, .statusFailed, .notConfigured, .notRunning]
}

struct SlotState: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let ok = SlotState(rawValue: "ok")
    static let drift = SlotState(rawValue: "drift")
    static let closed = SlotState(rawValue: "closed")
    static let unknown = SlotState(rawValue: "unknown")
    static let empty = SlotState(rawValue: "empty")
    static let allKnown: [SlotState] = [.ok, .drift, .closed, .unknown, .empty]
}

struct UsageSource: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let live = UsageSource(rawValue: "live")
    static let cache = UsageSource(rawValue: "cache")
    static let none = UsageSource(rawValue: "none")
    static let allKnown: [UsageSource] = [.live, .cache, .none]
}

struct UsageErrorCode: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let login = UsageErrorCode(rawValue: "login")
    static let mismatch = UsageErrorCode(rawValue: "mismatch")
    static let route = UsageErrorCode(rawValue: "route")
    static let port = UsageErrorCode(rawValue: "port")
    static let region = UsageErrorCode(rawValue: "region")
    static let trace = UsageErrorCode(rawValue: "trace")
    static let unsupported = UsageErrorCode(rawValue: "unsupported")
    static let timeout = UsageErrorCode(rawValue: "timeout")
    static let nolimits = UsageErrorCode(rawValue: "nolimits")
    /// 登录凭证失效了（改过密码、在别处退出过）：本地看着还登录着，要重新登录
    static let auth = UsageErrorCode(rawValue: "auth")
    static let error = UsageErrorCode(rawValue: "error")
    static let allKnown: [UsageErrorCode] = [.login, .mismatch, .route, .port, .region, .trace, .unsupported, .timeout, .nolimits, .auth, .error]
}

struct HealthSource: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let launch = HealthSource(rawValue: "launch")
    static let check = HealthSource(rawValue: "check")
    static let allKnown: [HealthSource] = [.launch, .check]
}

struct HealthKind: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let exit = HealthKind(rawValue: "exit")
    static let api = HealthKind(rawValue: "api")
    static let allKnown: [HealthKind] = [.exit, .api]
}

struct HealthResult: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let ok = HealthResult(rawValue: "ok")
    static let lowPurity = HealthResult(rawValue: "low_purity")
    static let restricted = HealthResult(rawValue: "restricted")
    static let noExit = HealthResult(rawValue: "no_exit")
    static let noScore = HealthResult(rawValue: "no_score")
    static let portClosed = HealthResult(rawValue: "port_closed")
    static let apiOk = HealthResult(rawValue: "api_ok")
    static let apiUnreachable = HealthResult(rawValue: "api_unreachable")
    static let allKnown: [HealthResult] = [.ok, .lowPurity, .restricted, .noExit, .noScore, .portClosed, .apiOk, .apiUnreachable]
}

/// 泄露检测：`dns` 有 ok / leak / failed，`webrtc` 还有 note（STUN 看到的地址和出口不同，但不是中国大陆的：只作提醒）。
struct LeakState: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let ok = LeakState(rawValue: "ok")
    static let leak = LeakState(rawValue: "leak")
    static let note = LeakState(rawValue: "note")
    static let failed = LeakState(rawValue: "failed")
    static let allKnown: [LeakState] = [.ok, .leak, .note, .failed]
}

// MARK: 事件（PROTOCOL 4）

struct EventType: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let step = EventType(rawValue: "step")
    static let consent = EventType(rawValue: "consent")
    static let terminal = EventType(rawValue: "terminal")
    static let fail = EventType(rawValue: "fail")
    static let result = EventType(rawValue: "result")
    static let ask = EventType(rawValue: "ask")
    static let allKnown: [EventType] = [.step, .consent, .terminal, .fail, .result, .ask]
}

struct StepCode: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let sync = StepCode(rawValue: "sync")
    static let preflight = StepCode(rawValue: "preflight")
    static let prepare = StepCode(rawValue: "prepare")
    static let open = StepCode(rawValue: "open")
    static let allKnown: [StepCode] = [.sync, .preflight, .prepare, .open]
}

struct ResultCode: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let opened = ResultCode(rawValue: "opened")
    static let activated = ResultCode(rawValue: "activated")
    /// 提问会话做完了（PROTOCOL 4.6）。
    static let added = ResultCode(rawValue: "added")
    static let keySaved = ResultCode(rawValue: "key_saved")
    static let removed = ResultCode(rawValue: "removed")
    /// 账号表保存了（`cca [codex] edit` 的表单）。
    static let saved = ResultCode(rawValue: "saved")
    static let routerReady = ResultCode(rawValue: "router_ready")
    static let localReady = ResultCode(rawValue: "local_ready")
    static let allKnown: [ResultCode] = [.opened, .activated, .added, .keySaved, .removed, .saved, .routerReady, .localReady]
}

struct FailCode: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let error = FailCode(rawValue: "error")
    static let appMissing = FailCode(rawValue: "app_missing")
    static let apiAccount = FailCode(rawValue: "api_account")
    static let toolMissing = FailCode(rawValue: "tool_missing")
    static let tableInvalid = FailCode(rawValue: "table_invalid")
    static let syncFailed = FailCode(rawValue: "sync_failed")
    static let noRouter = FailCode(rawValue: "no_router")
    static let portClosed = FailCode(rawValue: "port_closed")
    static let localUnavailable = FailCode(rawValue: "local_unavailable")
    /// 在外面时本机 Clash 里没有这个节点。
    static let nodeMissing = FailCode(rawValue: "node_missing")
    static let settingsConflict = FailCode(rawValue: "settings_conflict")
    static let regionRestricted = FailCode(rawValue: "region_restricted")
    static let noExitIP = FailCode(rawValue: "no_exit_ip")
    static let lowPurity = FailCode(rawValue: "low_purity")
    static let notLoggedIn = FailCode(rawValue: "not_logged_in")
    static let loginMismatch = FailCode(rawValue: "login_mismatch")
    static let apiUnreachable = FailCode(rawValue: "api_unreachable")
    static let openFailed = FailCode(rawValue: "open_failed")
    static let noChrome = FailCode(rawValue: "no_chrome")
    static let invalidURL = FailCode(rawValue: "invalid_url")
    static let loginUnsupported = FailCode(rawValue: "login_unsupported")
    static let appProfileUnsupported = FailCode(rawValue: "app_profile_unsupported")
    /// 改别名（`cca rename`）：新别名不合规、和别的账号重了、第三方 API 账号不能去掉别名、键里没有邮箱的官方账号不能改。
    static let aliasInvalid = FailCode(rawValue: "alias_invalid")
    /// 改别名时找不到这个账号。
    static let accountMissing = FailCode(rawValue: "account_missing")
    /// 重置额度：没有能用的重置机会了。
    static let noResetCredit = FailCode(rawValue: "no_reset_credit")
    /// 重置额度：这个版本的 codex 不支持（升级后再试）。
    static let resetUnsupported = FailCode(rawValue: "reset_unsupported")
    /// 提问会话被取消了（App 关掉了答案管道）。
    static let cancelled = FailCode(rawValue: "cancelled")
    /// 删除账号：它的桌面 App 还开着。
    static let appRunning = FailCode(rawValue: "app_running")
    /// 拉不到维护者的映射（`cca policy pull --json`，上次拉到的照用）。
    static let policyUnavailable = FailCode(rawValue: "policy_unavailable")
    static let allKnown: [FailCode] = [.cancelled, .appRunning, .policyUnavailable, .error, .appMissing, .apiAccount, .toolMissing, .tableInvalid, .syncFailed, .noRouter,
                                       .portClosed, .localUnavailable, .nodeMissing, .settingsConflict, .regionRestricted,
                                       .noExitIP, .lowPurity, .notLoggedIn, .loginMismatch, .apiUnreachable, .openFailed,
                                       .appProfileUnsupported, .noChrome, .invalidURL, .loginUnsupported,
                                       .aliasInvalid, .accountMissing, .noResetCredit, .resetUnsupported]
}

/// `terminal.code`（`install` 不在其中：`hello.installed == false` 由 App 自己处理）。
struct TerminalCode: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let login = TerminalCode(rawValue: "login")
    static let routerSync = TerminalCode(rawValue: "router_sync")
    static let routerSetup = TerminalCode(rawValue: "router_setup")
    static let localSetup = TerminalCode(rawValue: "local_setup")
    static let apiKey = TerminalCode(rawValue: "api_key")
    /// 账号表有表单改不了的问题（不是 JSON、键写错了）：在终端里编辑。
    static let edit = TerminalCode(rawValue: "edit")
    static let allKnown: [TerminalCode] = [.login, .routerSync, .routerSetup, .localSetup, .apiKey, .edit]
}

struct ConsentItem: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let routerReload = ConsentItem(rawValue: "router_reload")
    static let routerRestart = ConsentItem(rawValue: "router_restart")
    static let lowPurity = ConsentItem(rawValue: "low_purity")
    static let codexFallbackOff = ConsentItem(rawValue: "codex_fallback_off")
    static let openClashVerge = ConsentItem(rawValue: "open_clash_verge")
    static let builtinCore = ConsentItem(rawValue: "builtin_core")
    /// 重置额度会用掉一次重置机会（范围是账号目录名，默认不同意）。
    static let resetCredit = ConsentItem(rawValue: "reset_credit")
    static let allKnown: [ConsentItem] = [.routerReload, .routerRestart, .lowPurity, .codexFallbackOff, .openClashVerge, .builtinCore,
                                          .resetCredit]
}

/// 重置额度的结果（PROTOCOL 4.5）：重置了 / 这一次早就重置过了 / 用量还不需要重置（没扣次数）/ 没有能用的重置机会。
struct ResetOutcome: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let reset = ResetOutcome(rawValue: "reset")
    static let alreadyRedeemed = ResetOutcome(rawValue: "already_redeemed")
    static let nothingToReset = ResetOutcome(rawValue: "nothing_to_reset")
    static let noCredit = ResetOutcome(rawValue: "no_credit")
    static let allKnown: [ResetOutcome] = [.reset, .alreadyRedeemed, .nothingToReset, .noCredit]
}

/// 检查更新跟着哪种版本（PROTOCOL 5.1）：`stable`（正式版只看正式版）| `beta`（测试版也看更新的测试版）。
struct UpdateChannel: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let stable = UpdateChannel(rawValue: "stable")
    static let beta = UpdateChannel(rawValue: "beta")
    static let allKnown: [UpdateChannel] = [.stable, .beta]
}

/// 检查更新的版本从哪来（PROTOCOL 5.1）：`releases`（GitHub 的发布列表，有更新说明）| `latest`（列表拿不到时退回 releases/latest：
/// 只看得到正式版、没有更新说明）。
struct UpdateSource: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let releases = UpdateSource(rawValue: "releases")
    static let latest = UpdateSource(rawValue: "latest")
    static let allKnown: [UpdateSource] = [.releases, .latest]
}

struct Severity: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let warn = Severity(rawValue: "warn")
    static let info = Severity(rawValue: "info")
    static let allKnown: [Severity] = [.warn, .info]
}
