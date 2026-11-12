import Foundation

/// 问用户同不同意（PROTOCOL 4.2）。App 里是 NSAlert；测试里按剧本回答。
protocol Prompter: Sendable {
    func confirm(_ consent: ConsentEvent) async -> Bool
}

/// 会改东西的 cca 命令（带事件文件、走重跑流程）。
enum CcaCommand: Sendable, Hashable {
    case usage(ToolID, key: String?)
    /// 自动刷新用量：只查到期的这几个账号，低优先级，最多同时查两个。
    case usagePoll(ToolID, keys: [String])
    case check(ToolID, key: String?)
    /// 自动检测出口：只测到期的这几个账号，低优先级。
    case checkPoll(ToolID, keys: [String])
    case app(ToolID, key: String)
    case web(ToolID, key: String)
    /// 控制台的账单页（`cca [codex] billing KEY`）：每月 API 额度和余额在那里看，检查和网页版一样。
    case billing(ToolID, key: String)
    /// claude.ai 的用量页（`cca limit-resets KEY`）：Claude 账号的重置次数和到期日在那里看，检查和网页版一样。
    case limitResets(ToolID, key: String)
    case routerSync
    /// 拉维护者的映射（`cca policy pull --json`，PROTOCOL 4.9）：照映射改账号表、同步路由器，要重载的不做（待同步）。manual 是用户点的，不降优先级。
    case policyPull(manual: Bool)
    /// 「更新并重新打开」的第一步：下载校验好和 cca 同版本的 App，先放着（PROTOCOL 5）。
    case guiUpdate
    /// 账号表按这个顺序排（`cca [codex] reorder 账号…`）：只改先后，端口、路由器都不动。
    case reorder(ToolID, keys: [String])
    /// 请求维护者把这个账号现在的节点锁进映射（`cca [codex] policy request KEY`，PROTOCOL 4.8）：stdout 是维护者直接运行的那条命令。
    case policyRequest(ToolID, key: String)
    /// 改别名（`cca [codex] rename KEY 新别名`，新别名是 - 时去掉别名）：stdout 是新的键。
    case rename(ToolID, key: String, alias: String)
    /// 重置额度（`cca codex reset KEY --json`，PROTOCOL 4.5）：要同意（reset_credit），stdout 是 kind=reset 的文档。
    case reset(ToolID, key: String)
    /// 「更新并重新打开」之前先把 cca 更新到这个版本（`cca update 版本号`，PROTOCOL 5.1）。
    case update(String)
    /// 检查更新（`cca update --check --json`，PROTOCOL 5.1）：只读、要联网，stdout 是 kind=update 的文档；查不了时看 fail 事件。
    case updateCheck
    /// 客户端（Claude、ChatGPT 桌面版，终端里的 Claude Code、Codex）更新：检查不安装，安装需经过绑定 token 的 consent。
    case clientCheck(force: Bool)
    case clientMode(automatic: Bool)
    case clientInstall(token: String)
    /// 账号连着的设备（`cca [codex] devices KEY --json`，PROTOCOL 3.4）：只读、要联网。auto 是账号页开着时的定时刷新（低优先级）。
    case devices(ToolID, key: String, auto: Bool)
    /// 读这个账号存的 API key（`cca [codex] key KEY --show`，PROTOCOL 4.10）：stdout 原样是 key。**输出只经内存里的管道**，
    /// 不能是 runsToCompletion（那种的 stdout 写进文件）；用完马上丢，不进被观察的状态、不进提示、不进读屏。
    case keyShow(ToolID, key: String)
    /// 删掉官方账号记下的 API key（`cca [codex] key KEY --forget`）：要同意（key_forget，默认不同意）。
    case keyForget(ToolID, key: String)

    var tool: ToolID? {
        switch self {
        case .usage(let t, _), .usagePoll(let t, _), .check(let t, _), .checkPoll(let t, _), .app(let t, _), .web(let t, _), .billing(let t, _), .limitResets(let t, _), .rename(let t, _, _), .reorder(let t, _), .reset(let t, _), .policyRequest(let t, _), .devices(let t, _, _), .keyShow(let t, _), .keyForget(let t, _): return t
        case .routerSync, .policyPull, .guiUpdate, .update, .updateCheck, .clientCheck, .clientMode, .clientInstall: return nil
        }
    }

    var key: String? {
        switch self {
        case .usage(_, let k), .check(_, let k): return k
        case .app(_, let k), .web(_, let k), .billing(_, let k), .limitResets(_, let k), .rename(_, let k, _), .reset(_, let k), .policyRequest(_, let k), .devices(_, let k, _), .keyShow(_, let k), .keyForget(_, let k): return k
        case .usagePoll, .checkPoll, .reorder, .routerSync, .policyPull, .guiUpdate, .update, .updateCheck, .clientCheck, .clientMode, .clientInstall: return nil
        }
    }

    /// 后台的活：低优先级运行。
    var isBackground: Bool {
        switch self {
        case .usagePoll, .checkPoll, .policyPull(manual: false), .clientCheck(force: false), .devices(_, _, auto: true): return true
        default: return false
        }
    }

    var extraEnvironment: [String: String] {
        if case .usagePoll = self { return ["CCA_USAGE_JOBS": "2"] }
        return [:]
    }

    /// `cca` 后面的参数（工具的前缀见 ToolID.defaultPrefix）。
    var arguments: [String] {
        let prefix = tool?.defaultPrefix ?? []
        switch self {
        case .usage(_, let k): return prefix + ["usage"] + (k.map { [$0] } ?? []) + ["--json"]
        case .usagePoll(_, let keys): return prefix + ["usage"] + keys + ["--json"]
        case .check(_, let k): return prefix + ["check"] + (k.map { [$0] } ?? []) + ["--json"]
        case .checkPoll(_, let keys): return prefix + ["check"] + keys + ["--json"]
        case .app(_, let k): return prefix + ["app", k]
        case .web(_, let k): return prefix + ["web", k]
        case .billing(_, let k): return prefix + ["billing", k]
        case .limitResets(_, let k): return prefix + ["limit-resets", k]
        case .routerSync: return ["router", "sync"]
        case .policyPull: return ["policy", "pull", "--json"]
        case .guiUpdate: return ["gui", "update"]
        case .rename(_, let k, let a): return prefix + ["rename", k, a]
        case .reorder(_, let keys): return prefix + ["reorder"] + keys
        case .policyRequest(_, let k): return prefix + ["policy", "request", k]
        case .reset(_, let k): return prefix + ["reset", k, "--json"]
        case .update(let v): return ["update", v]
        case .updateCheck: return ["update", "--check", "--json"]
        case .clientCheck(let force): return ["clients", "check"] + (force ? ["--force"] : []) + ["--json"]
        case .clientMode(let automatic): return ["clients", "mode", automatic ? "auto" : "off", "--json"]
        case .clientInstall(let token): return ["clients", "install", token, "--json"]
        case .devices(_, let k, _): return prefix + ["devices", k, "--json"]
        case .keyShow(_, let k): return prefix + ["key", k, "--show"]
        case .keyForget(_, let k): return prefix + ["key", k, "--forget"]
        }
    }

    /// PROTOCOL 1 建议的超时（runsToCompletion 的命令不限时）。
    var timeout: TimeInterval {
        switch self {
        case .usage, .usagePoll: return 150
        case .check: return 180
        case .checkPoll: return 300
        case .app, .web, .billing, .limitResets: return 120
        case .routerSync, .policyPull: return 180
        case .guiUpdate, .update: return 300
        case .rename, .reorder, .policyRequest: return 60
        case .reset, .updateCheck: return 150
        case .clientCheck, .clientInstall: return 900
        case .clientMode, .devices: return 30
        case .keyShow, .keyForget: return 40   // cca 读钥匙串最多等 30 秒
        }
    }

    /// 一旦开始就要跑完（PROTOCOL 1）：输出接文件，不限时，App 退出时也不杀。重置额度也是：同意之后就会用掉一次，
    /// 半路杀掉可能用掉了却没记下重置后的用量。
    var runsToCompletion: Bool {
        switch self {
        case .app, .web, .billing, .limitResets, .routerSync, .reset, .clientInstall: return true
        case .usage, .usagePoll, .check, .checkPoll, .policyPull, .guiUpdate, .rename, .reorder, .policyRequest, .update, .updateCheck, .clientCheck, .clientMode, .devices,
             .keyShow, .keyForget: return false
        }
    }

    /// 同一个闸门里的命令一次只跑一个：同一个账号的 app / web；同一个工具的 usage；同一个账号的 reset；
    /// router sync 全局一个；更新 cca、gui update 全局一个（一个接着一个）。
    var gate: String? {
        switch self {
        case .app(let t, let k), .web(let t, let k), .billing(let t, let k), .limitResets(let t, let k), .rename(let t, let k, _): return "launch:\(t.rawValue):\(k)"
        case .usage(let t, _), .usagePoll(let t, _): return "usage:\(t.rawValue)"
        case .reorder(let t, _): return "reorder:\(t.rawValue)"   // 连着拖几次：一个接一个，最后拖的那次写在最后
        case .reset(let t, let k): return "reset:\(t.rawValue):\(k)"
        case .routerSync, .policyPull: return "router-sync"   // 拉映射时可能同步路由器：一个接一个
        case .guiUpdate, .update: return "gui-update"
        case .clientCheck, .clientMode, .clientInstall: return "client-update"
        case .check, .checkPoll, .updateCheck, .policyRequest, .devices: return nil   // devices：AppModel 已保证同一个账号一次一个
        case .keyShow(let t, let k), .keyForget(let t, let k): return "key:\(t.rawValue):\(k)"
        }
    }
}

/// 只读查询出错。
enum CcaError: Error, CustomStringConvertible, Equatable {
    case launch(String)
    case timedOut(TimeInterval)
    case cancelled
    case exit(Int32, stderrTail: String)
    case badOutput(String, stderrTail: String)
    case codexDisabled

    var description: String {
        switch self {
        case .launch(let s): return s
        case .timedOut(let t): return "cca 在 \(Int(t)) 秒内没有完成"
        case .cancelled: return "已取消"
        case .exit(let c, let tail): return tail.isEmpty ? "cca 出错了（退出码 \(c)）" : "cca 出错了（退出码 \(c)）：\(tail)"
        case .badOutput(let s, _): return s
        case .codexDisabled: return "还没启用这种账号"
        }
    }
}

/// 失败的原因（给界面挑文案和补救按钮）。
enum FailureReason: Sendable, Equatable {
    /// cca 报的失败（最后一条 fail 事件，或者没有事件时的退出码）。
    case command
    /// 同一个授权被再次要求（PROTOCOL 4.3 第 6 条）。
    case loopGuard
    /// 重跑了 3 轮还要授权。
    case tooManyRounds
    case launch
    case timedOut
    case cancelled
    case codexDisabled
}

struct Failure: Sendable, Equatable {
    var reason: FailureReason
    /// 最后一条 fail 事件的 code。
    var code: FailCode?
    var message: String
    /// stderr 的最后几行（“详情”）。
    var stderrTail: String
    var exitCode: Int32?
    var tool: ToolID?
    var key: String?
}

enum ExecStatus: Sendable, Equatable {
    case success
    /// 用户没同意：停下，什么都没改（不是错误）。
    case refused(ConsentEvent)
    /// 要同意，但这次调用不弹框（例如后台自动刷新用量）。
    case needsConsent([ConsentEvent])
    /// 要在终端里完成。
    case terminal(TerminalEvent)
    case failed(Failure)
}

struct ExecResult: Sendable {
    var status: ExecStatus
    /// 最后一次运行的输出。
    var last: CcaResponse?
    var runs: Int
    var grants: [String]

    var resultEvent: ResultEvent? { last?.events.compactMap(\.result).last }
    var isSuccess: Bool { status == .success }
    var failure: Failure? { if case .failed(let f) = status { return f }; return nil }
}

/// 先来先得的异步锁。
actor AsyncGate {
    private var busy = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func acquire() async {
        if !busy {
            busy = true
            return
        }
        await withCheckedContinuation { waiters.append($0) }
    }

    func release() {
        if waiters.isEmpty {
            busy = false
        } else {
            waiters.removeFirst().resume()
        }
    }
}

/// 和 cca 打交道的唯一入口（PROTOCOL 全文）。
actor CcaClient {
    let transport: Transport
    /// 最多重跑几轮（PROTOCOL 4.3 第 6 条）。
    let maxReruns = 3
    private var gates: [String: AsyncGate] = [:]
    /// 能调用的工具（PROTOCOL 1）：Claude 一直能；`cca codex …`、`cca agy …` 只在 hello 说启用了时才能（不然会建出账号表）。
    private(set) var enabledTools: Set<ToolID> = [.claude]
    var codexEnabled: Bool { enabledTools.contains(.codex) }

    init(transport: Transport) {
        self.transport = transport
    }

    /// 按最新的 hello 更新：哪些工具的命令能调用。
    func setEnabledTools(_ tools: [ToolID]) {
        enabledTools = Set(tools).union([.claude])
    }

    /// 只改 Codex 的（测试用）。
    func setCodexEnabled(_ enabled: Bool) {
        if enabled { enabledTools.insert(.codex) } else { enabledTools.remove(.codex) }
    }

    private func gate(_ name: String) -> AsyncGate {
        if let g = gates[name] { return g }
        let g = AsyncGate()
        gates[name] = g
        return g
    }

    // MARK: 只读查询（PROTOCOL 2）

    func hello() async throws -> HelloDoc {
        try await query(HelloDoc.self, ["_gui", "hello"], timeout: 5)
    }

    func state() async throws -> StateDoc {
        try await query(StateDoc.self, ["_gui", "state"], timeout: 15)
    }

    func clientUpdates() async throws -> ClientUpdatesDoc {
        try await query(ClientUpdatesDoc.self, ["clients", "status", "--json"], timeout: 30)
    }

    /// route 为 nil 时按 route_pref；给了就用 CCA_ROUTE 看指定的一边。
    func live(route: Route? = nil) async throws -> LiveDoc {
        var extra: [String: String] = [:]
        if let route, route == .router || route == .local { extra["CCA_ROUTE"] = route.rawValue }
        return try await query(LiveDoc.self, ["_gui", "live"], timeout: 30, extra: extra)
    }

    private func query<T: CcaDocument>(_ type: T.Type, _ args: [String], timeout: TimeInterval,
                                       extra: [String: String] = [:]) async throws -> T {
        let resp: CcaResponse
        do {
            resp = try await transport.send(CcaRequest(arguments: args, timeout: timeout, extraEnvironment: extra), onEvent: nil)
        } catch {
            throw CcaError.launch(String(describing: error))
        }
        let tail = resp.stderrText.lastLines(8)
        if resp.cancelled { throw CcaError.cancelled }
        if resp.timedOut { throw CcaError.timedOut(timeout) }
        guard resp.exitCode == 0 else { throw CcaError.exit(resp.exitCode, stderrTail: tail) }
        do {
            return try DocumentDecoder.decode(T.self, from: resp.stdout)
        } catch {
            throw CcaError.badOutput(String(describing: error), stderrTail: tail)
        }
    }

    // MARK: 提问会话（PROTOCOL 4.6）

    /// App 同一时间只开一个流程，不用闸门；不限时（用户答题要多久就多久）。
    nonisolated func startSession(_ arguments: [String]) throws -> CcaSession {
        try transport.startSession(arguments)
    }

    // MARK: 会改东西的命令（PROTOCOL 4）

    /// 没有 JSON 输出的命令（app、web、router sync、gui update）。
    func execute(_ command: CcaCommand, prompter: Prompter?,
                 onEvent: (@Sendable (CcaEvent) -> Void)? = nil) async -> ExecResult {
        await gated(command, prompter: prompter, decode: { _ in Bool?.none }, onEvent: onEvent).1
    }

    /// 输出 JSON 的命令（usage、check）：stdout 的 JSON 能解码就以它为准（例如 usage 退出码 1：没有一个账号有数据）。
    /// 每次运行只解码一次；返回最后一次运行解码出来的文档。
    func document<T: CcaDocument>(_ command: CcaCommand, as type: T.Type, prompter: Prompter?,
                                  onEvent: (@Sendable (CcaEvent) -> Void)? = nil) async -> (T?, ExecResult) {
        await gated(command, prompter: prompter, decode: { try? DocumentDecoder.decode(T.self, from: $0.stdout) }, onEvent: onEvent)
    }

    /// 同一个闸门里的命令排队（整个流程，包括弹框和重跑，都在闸门里）。
    private func gated<T: Sendable>(_ command: CcaCommand, prompter: Prompter?, decode: @escaping @Sendable (CcaResponse) -> T?,
                                    onEvent: (@Sendable (CcaEvent) -> Void)?) async -> (T?, ExecResult) {
        if let tool = command.tool, !enabledTools.contains(tool) {
            let f = Failure(reason: .codexDisabled, code: nil, message: "还没启用 \(Texts.product(tool)) 账号", stderrTail: "",
                            exitCode: nil, tool: tool, key: command.key)
            return (nil, ExecResult(status: .failed(f), last: nil, runs: 0, grants: []))
        }
        guard let name = command.gate else {
            return await rounds(command, prompter: prompter, decode: decode, onEvent: onEvent)
        }
        let g = gate(name)
        await g.acquire()
        let r = await rounds(command, prompter: prompter, decode: decode, onEvent: onEvent)
        await g.release()
        return r
    }

    /// PROTOCOL 4.3 的重跑流程。prompter 为 nil 表示不弹框：要同意时返回 `.needsConsent`。
    private func rounds<T: Sendable>(_ command: CcaCommand, prompter: Prompter?, decode: @Sendable (CcaResponse) -> T?,
                                     onEvent: (@Sendable (CcaEvent) -> Void)?) async -> (T?, ExecResult) {
        var grants: [String] = []
        var runs = 0
        func result(_ status: ExecStatus, _ resp: CcaResponse?, _ doc: T?) -> (T?, ExecResult) {
            (doc, ExecResult(status: status, last: resp, runs: runs, grants: grants))
        }
        func fail(_ reason: FailureReason, _ message: String, _ resp: CcaResponse?, _ doc: T? = nil) -> (T?, ExecResult) {
            let f = Failure(reason: reason, code: resp?.events.compactMap(\.fail).last?.code, message: message,
                            stderrTail: resp?.stderrText.lastLines(12) ?? "", exitCode: resp?.exitCode,
                            tool: command.tool, key: command.key)
            return result(.failed(f), resp, doc)
        }
        while true {
            runs += 1
            let req = CcaRequest(arguments: command.arguments, timeout: command.timeout, wantsEvents: true, grants: grants,
                                 extraEnvironment: command.extraEnvironment, runsToCompletion: command.runsToCompletion, background: command.isBackground)
            let resp: CcaResponse
            do {
                resp = try await transport.send(req, onEvent: onEvent)
            } catch {
                return fail(.launch, "没能运行 cca：\(error)", nil)
            }
            let doc = decode(resp)
            if resp.cancelled { return fail(.cancelled, "已取消", resp, doc) }
            if resp.timedOut { return fail(.timedOut, "cca 在 \(Int(command.timeout)) 秒内没有完成，已停止", resp, doc) }
            if resp.exitCode == 0 { return result(.success, resp, doc) }
            // 同一个授权被再次要求：停下（防止死循环）
            var seen = Set<String>()
            let consents = resp.events.compactMap(\.consent).filter { seen.insert($0.grant).inserted }
            if let again = consents.first(where: { grants.contains($0.grant) }) {
                let what = again.title.isEmpty ? again.grant : again.title
                return fail(.loopGuard, "已经同意过「\(what)」，cca 仍然要求同意，已停止", resp, doc)
            }
            if !consents.isEmpty {
                guard let prompter else { return result(.needsConsent(consents), resp, doc) }
                if runs > maxReruns {
                    return fail(.tooManyRounds, "重跑了 \(maxReruns) 次仍然需要同意，已停止", resp, doc)
                }
                for c in consents {
                    if !(await prompter.confirm(c)) { return result(.refused(c), resp, doc) }
                }
                grants.append(contentsOf: consents.map(\.grant))
                continue
            }
            if let t = resp.events.compactMap(\.terminal).last { return result(.terminal(t), resp, doc) }
            if doc != nil { return result(.success, resp, doc) }
            let lastFail = resp.events.compactMap(\.fail).last
            return fail(.command, nonEmpty(lastFail?.message) ?? "cca 没有完成（退出码 \(resp.exitCode)）", resp)
        }
    }

    // MARK: 更新（PROTOCOL 5 的第二步）

    /// 新版本已经下载好之后：脱离 App 运行 `cca gui update --relaunch <pid>`（不设 CCA_EVENTS、不限时，输出接到文件）。
    /// 它等 App 退出后换上新版本再打开；App 随后自己退出。
    func relaunchAfterUpdate(pid: Int32) throws {
        try transport.spawnDetached(CcaRequest(arguments: ["gui", "update", "--relaunch", String(pid)], timeout: 300))
    }
}
