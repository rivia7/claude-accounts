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
    static let unset = LocalClient(rawValue: "")
    static let allKnown: [LocalClient] = [.clashVerge, .manual, .unset]
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
    static let error = UsageErrorCode(rawValue: "error")
    static let allKnown: [UsageErrorCode] = [.login, .mismatch, .route, .port, .region, .trace, .unsupported, .timeout, .nolimits, .error]
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

/// 泄露检测：`dns` 有 ok / leak / failed，`webrtc` 有 ok / leak。
struct LeakState: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let ok = LeakState(rawValue: "ok")
    static let leak = LeakState(rawValue: "leak")
    static let failed = LeakState(rawValue: "failed")
    static let allKnown: [LeakState] = [.ok, .leak, .failed]
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
    static let allKnown: [EventType] = [.step, .consent, .terminal, .fail, .result]
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
    static let allKnown: [ResultCode] = [.opened, .activated]
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
    static let appProfileUnsupported = FailCode(rawValue: "app_profile_unsupported")
    /// 改别名（`cca rename`）：新别名不合规、和别的账号重了、第三方 API 账号不能去掉别名、键里没有邮箱的官方账号不能改。
    static let aliasInvalid = FailCode(rawValue: "alias_invalid")
    /// 改别名时找不到这个账号。
    static let accountMissing = FailCode(rawValue: "account_missing")
    static let allKnown: [FailCode] = [.error, .appMissing, .apiAccount, .toolMissing, .tableInvalid, .syncFailed, .noRouter,
                                       .portClosed, .localUnavailable, .nodeMissing, .settingsConflict, .regionRestricted,
                                       .noExitIP, .lowPurity, .notLoggedIn, .loginMismatch, .apiUnreachable, .openFailed,
                                       .appProfileUnsupported, .aliasInvalid, .accountMissing]
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
    static let allKnown: [TerminalCode] = [.login, .routerSync, .routerSetup, .localSetup, .apiKey]
}

struct ConsentItem: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let routerReload = ConsentItem(rawValue: "router_reload")
    static let routerRestart = ConsentItem(rawValue: "router_restart")
    static let lowPurity = ConsentItem(rawValue: "low_purity")
    static let codexFallbackOff = ConsentItem(rawValue: "codex_fallback_off")
    static let openClashVerge = ConsentItem(rawValue: "open_clash_verge")
    static let allKnown: [ConsentItem] = [.routerReload, .routerRestart, .lowPurity, .codexFallbackOff, .openClashVerge]
}

struct Severity: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let warn = Severity(rawValue: "warn")
    static let info = Severity(rawValue: "info")
    static let allKnown: [Severity] = [.warn, .info]
}
