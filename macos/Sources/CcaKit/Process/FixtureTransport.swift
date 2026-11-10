import Foundation

/// 演示 / 测试：不运行 cca，按命令从一个目录里取固定的输出（环境变量 CCA_GUI_FIXTURES=<目录> 时 App 用它）。
///
/// | 命令 | 文件 | 换成别的文件 |
/// |---|---|---|
/// | `_gui hello` | hello.json | CCA_GUI_FIXTURE_HELLO |
/// | `_gui state` | state.json | CCA_GUI_FIXTURE_STATE |
/// | `_gui live` | live_router.json（CCA_ROUTE=local 时 live_local.json） | CCA_GUI_FIXTURE_LIVE |
/// | `usage --json` | usage_claude.json / usage_codex.json | — |
/// | `check KEY --json` | check.json | — |
/// | `app KEY` / `web KEY` / `billing KEY` / `limit-resets KEY` | events_open.jsonl | CCA_GUI_FIXTURE_APP / CCA_GUI_FIXTURE_WEB |
/// | `router sync` | events_sync.jsonl（同意后成功） | CCA_GUI_FIXTURE_SYNC |
/// | `codex reset KEY --json` | events_reset_consent.jsonl（要同意），同意后 reset.json | — |
/// | `update --check --json` | update.json | — |
/// | `clients status/check --json` | client_updates.json | — |
/// | `devices KEY --json` | devices.json（键换成这次问的账号） | — |
///
/// 事件文件的退出码：有没授权的 consent → 3；有 fail / terminal → 1；否则 0。consent 都授权了就当作成功
/// （去掉 consent 和 fail）。`update 版本号` 什么都不做（`gui update` 演示模式下 App 不会调用）。
///
/// 演示模式（App 用 CCA_GUI_FIXTURES 打开）传 rebaseTo = 现在：夹具里的时间（`generated_at`、`checked_at`、`*_epoch`）
/// 一起平移，让 state.json 的 generated_at 落在“现在”——几点重置、几点回来、多久前检测的，永远像刚生成的一样。
/// 测试不传，读到的是原样的文件。
struct FixtureTransport: Transport {
    let directory: URL
    let stepDelay: TimeInterval
    let overrides: [String: String]
    /// 时间平移多少秒（不平移是 0）
    let shift: Double

    var isFixture: Bool { true }

    init(directory: URL, environment: [String: String] = [:], stepDelay: TimeInterval = 0.35, rebaseTo now: Double? = nil) {
        self.directory = directory
        self.stepDelay = stepDelay
        var o: [String: String] = [:]
        for (name, v) in [("hello", "CCA_GUI_FIXTURE_HELLO"), ("state", "CCA_GUI_FIXTURE_STATE"), ("live", "CCA_GUI_FIXTURE_LIVE"),
                          ("app", "CCA_GUI_FIXTURE_APP"), ("web", "CCA_GUI_FIXTURE_WEB"), ("sync", "CCA_GUI_FIXTURE_SYNC")] {
            if let f = environment[v], !f.isEmpty { o[name] = f }
        }
        overrides = o
        let anchor = (try? Data(contentsOf: directory.appendingPathComponent(o["state"] ?? "state.json")))
            .flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
            .flatMap { ($0["generated_at"] as? NSNumber)?.doubleValue }
        shift = now.flatMap { n in anchor.map { n - $0 } } ?? 0
    }

    /// 把 JSON 里的时间平移 by 秒：`generated_at`、`checked_at` 和以 `_epoch` 结尾的数字（ISO 写法的 `*_at` 字符串、
    /// `age_seconds` 不动）。读不懂就原样返回。
    static func rebase(_ data: Data, by delta: Double) -> Data {
        guard delta != 0, let obj = try? JSONSerialization.jsonObject(with: data) else { return data }
        func walk(_ x: Any) -> Any {
            if let d = x as? [String: Any] {
                var out: [String: Any] = [:]
                for (k, v) in d {
                    if k == "generated_at" || k == "checked_at" || k.hasSuffix("_epoch"), let n = v as? NSNumber,
                       CFGetTypeID(n) != CFBooleanGetTypeID() {
                        out[k] = n.doubleValue + delta
                    } else {
                        out[k] = walk(v)
                    }
                }
                return out
            }
            if let a = x as? [Any] { return a.map(walk) }
            return x
        }
        return (try? JSONSerialization.data(withJSONObject: walk(obj))) ?? data
    }

    private func read(_ name: String) -> Data? {
        guard let d = try? Data(contentsOf: directory.appendingPathComponent(name)) else { return nil }
        return name.hasSuffix(".json") ? Self.rebase(d, by: shift) : d
    }

    /// 演示时假装慢一点（看得到“刷新中…”这些进度）；测试里 stepDelay 是 0，不等
    private func pause(_ steps: Double) async {
        if stepDelay > 0 { try? await Task.sleep(nanoseconds: UInt64(stepDelay * steps * 1_000_000_000)) }
    }

    private func missing(_ name: String) -> CcaResponse {
        CcaResponse(exitCode: 1, stdout: Data(), stderr: Data("演示数据里没有 \(name)\n".utf8), events: [])
    }

    private func json(_ name: String, exitCode: Int32 = 0) -> CcaResponse {
        guard let d = read(name) else { return missing(name) }
        return CcaResponse(exitCode: exitCode, stdout: d, stderr: Data(), events: [])
    }

    private func events(_ name: String, grants: [String], onEvent: (@Sendable (CcaEvent) -> Void)?) async -> CcaResponse {
        guard let d = read(name) else { return missing(name) }
        var evs = EventLog.parse(d)
        let consents = evs.compactMap(\.consent)
        let granted = !consents.isEmpty && consents.allSatisfy { grants.contains($0.grant) }
        if granted {
            evs.removeAll { $0.consent != nil || $0.fail != nil }
        }
        for e in evs {
            await pause(1)
            onEvent?(e)
        }
        let code: Int32
        if !granted && !consents.isEmpty {
            code = 3
        } else if evs.contains(where: { $0.fail != nil || $0.terminal != nil }) {
            code = 1
        } else {
            code = 0
        }
        let stderr = evs.compactMap { $0.fail?.message ?? $0.terminal?.message ?? $0.result?.message }.joined(separator: "\n")
        return CcaResponse(exitCode: code, stdout: Data(), stderr: Data(stderr.utf8), events: evs)
    }

    func send(_ request: CcaRequest, onEvent: (@Sendable (CcaEvent) -> Void)?) async throws -> CcaResponse {
        var args = request.arguments
        var tool = ToolID.claude
        if args.first == "codex" {
            tool = .codex
            args.removeFirst()
        }
        let verb = args.first ?? "", sub = args.dropFirst().first ?? ""
        switch (verb, sub) {
        case ("_gui", "hello"):
            return json(overrides["hello"] ?? "hello.json")
        case ("_gui", "state"):
            return json(overrides["state"] ?? "state.json")
        case ("_gui", "live"):
            let local = request.extraEnvironment["CCA_ROUTE"] == "local"
            return json(overrides["live"] ?? (local ? "live_local.json" : "live_router.json"))
        case ("usage", _):
            await pause(3)
            let name = tool == .codex ? "usage_codex.json" : "usage_claude.json"
            guard let d = read(name) else { return missing(name) }
            let doc = try? DocumentDecoder.decode(UsageDoc.self, from: d)
            let any = doc?.accounts.contains { $0.source != .none } ?? false
            return CcaResponse(exitCode: any ? 0 : 1, stdout: d, stderr: Data(), events: [])
        case ("check", _):
            await pause(3)
            return json("check.json")
        case ("app", _):
            return await events(overrides["app"] ?? "events_open.jsonl", grants: request.grants, onEvent: onEvent)
        case ("web", _), ("billing", _), ("limit-resets", _):
            return await events(overrides["web"] ?? "events_open.jsonl", grants: request.grants, onEvent: onEvent)
        case ("router", "sync"):
            return await events(overrides["sync"] ?? "events_sync.jsonl", grants: request.grants, onEvent: onEvent)
        case ("reset", _):
            let asked = await events("events_reset_consent.jsonl", grants: request.grants, onEvent: onEvent)
            guard asked.exitCode == 0 else { return asked }
            await pause(3)
            return json("reset.json")
        case ("update", "--check"):
            return json("update.json")
        case ("clients", "status"), ("clients", "check"):
            return json("client_updates.json")
        case ("devices", let key):
            guard let d = read("devices.json") else { return missing("devices.json") }
            return CcaResponse(exitCode: 0, stdout: Self.devicesFor(key, d), stderr: Data(), events: [])
        case ("update", _):
            return CcaResponse(exitCode: 0, stdout: Data(), stderr: Data(), events: [])
        default:
            let cmd = (["cca"] + request.arguments).joined(separator: " ")
            return CcaResponse(exitCode: 2, stdout: Data(), stderr: Data("演示模式不支持：\(cmd)\n".utf8), events: [])
        }
    }

    func spawnDetached(_ request: CcaRequest) throws {}

    /// 演示数据里只有一个账号的设备：换成这次问的账号（每个账号页都有东西看）
    static func devicesFor(_ key: String, _ data: Data) -> Data {
        guard var j = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              var accounts = j["accounts"] as? [[String: Any]], !accounts.isEmpty else { return data }
        accounts = [accounts[0].merging(["key": key]) { $1 }]
        j["accounts"] = accounts
        return (try? JSONSerialization.data(withJSONObject: j)) ?? data
    }

    /// 提问会话的演示：按 `flow_<参数用 _ 连起来>.flow` 回放（没有这个文件就和不支持一样）。
    func startSession(_ arguments: [String]) throws -> CcaSession {
        var args = arguments
        if args.first == "codex" { args.removeFirst() }
        if args.first == "key" || args.first == "rm" { args = [args[0]] }
        let name = "flow_" + args.joined(separator: "_") + ".flow"
        guard let d = read(name) else { throw SessionUnsupported() }
        return DemoSession(data: d)
    }
}

/// 演示用的提问会话：一行一个事件，`ask` 之后等答案；`{"pause":true}` 停在那里，`{"exit":N}` 记下退出码。
final class DemoSession: CcaSession, @unchecked Sendable {
    let updates: AsyncStream<SessionUpdate>
    private let continuation: AsyncStream<SessionUpdate>.Continuation
    private let lock = NSLock()
    private var rows: [Data]
    private var exitCode: Int32 = 0
    private var ended = false

    init(data: Data) {
        var c: AsyncStream<SessionUpdate>.Continuation!
        updates = AsyncStream(bufferingPolicy: .unbounded) { c = $0 }
        continuation = c
        rows = data.split(separator: 0x0A, omittingEmptySubsequences: true).map { Data($0) }
        play()
    }

    private func play() {
        lock.lock()
        defer { lock.unlock() }
        while !ended, !rows.isEmpty {
            let row = rows.removeFirst()
            let obj = try? JSONSerialization.jsonObject(with: row) as? [String: Any]
            if obj?["pause"] != nil { return }
            if let code = obj?["exit"] as? Int {
                exitCode = Int32(code)
                continue
            }
            guard let e = EventLog.parseLine(row) else { continue }
            continuation.yield(.event(e))
            if case .ask = e { return }
        }
        if !ended { finish(exitCode) }
    }

    private func finish(_ code: Int32) {
        ended = true
        continuation.yield(.finished(CcaResponse(exitCode: code, stdout: Data(), stderr: Data(), events: [])))
        continuation.finish()
    }

    func answer(_ lines: [String]) { play() }

    func closeAnswers() {
        lock.withLock {
            guard !ended else { return }
            finish(143)
        }
    }

    func stop() { closeAnswers() }
}
