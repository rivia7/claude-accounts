import Foundation

// MARK: 脚本化的 transport / prompter

final class ScriptedTransport: Transport, @unchecked Sendable {
    typealias Handler = @Sendable (CcaRequest, Int) -> CcaResponse
    private let handler: Handler
    private let delay: TimeInterval
    private let lock = NSLock()
    private var _requests: [CcaRequest] = []
    private var _detached: [CcaRequest] = []
    private var _log: [(label: String, start: Double, end: Double)] = []

    var isFixture: Bool { false }

    init(delay: TimeInterval = 0, handler: @escaping Handler) {
        self.delay = delay
        self.handler = handler
    }

    var requests: [CcaRequest] { lock.lock(); defer { lock.unlock() }; return _requests }
    var detached: [CcaRequest] { lock.lock(); defer { lock.unlock() }; return _detached }
    var log: [(label: String, start: Double, end: Double)] { lock.lock(); defer { lock.unlock() }; return _log }

    func send(_ request: CcaRequest, onEvent: (@Sendable (CcaEvent) -> Void)?) async throws -> CcaResponse {
        let i = lock.withLock { () -> Int in
            _requests.append(request)
            return _requests.count - 1
        }
        let start = Date().timeIntervalSince1970
        let resp = handler(request, i)
        for e in resp.events { onEvent?(e) }
        if delay > 0 { try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000)) }
        let end = Date().timeIntervalSince1970
        lock.withLock { _log.append((request.arguments.joined(separator: " "), start, end)) }
        return resp
    }

    func spawnDetached(_ request: CcaRequest) throws {
        lock.lock()
        _detached.append(request)
        lock.unlock()
    }

    /// 内置代理的内核：记下每次启动，给一个假的进程句柄
    private var _coreHandles: [FakeCoreHandle] = []
    var coreHandles: [FakeCoreHandle] { lock.withLock { _coreHandles } }
    var coreStarts: Int { coreHandles.count }

    func startCore(logPath: String, onExit: @escaping @Sendable (Int32) -> Void) throws -> CoreHandle {
        let h = FakeCoreHandle()
        lock.withLock { _coreHandles.append(h) }
        return h
    }
}

/// 测试里会换的一段文字（线程安全）。
final class TextBox: @unchecked Sendable {
    private let lock = NSLock()
    private var _value: String
    init(_ v: String) { _value = v }
    var value: String { lock.withLock { _value } }
    func set(_ v: String) { lock.withLock { _value = v } }
}

final class ScriptedPrompter: Prompter, @unchecked Sendable {
    private let lock = NSLock()
    private var answers: [Bool]
    private var _asked: [ConsentEvent] = []

    init(_ answers: [Bool]) {
        self.answers = answers
    }

    var asked: [ConsentEvent] { lock.lock(); defer { lock.unlock() }; return _asked }

    func confirm(_ consent: ConsentEvent) async -> Bool {
        lock.withLock {
            _asked.append(consent)
            return answers.isEmpty ? false : answers.removeFirst()
        }
    }
}

final class EventSink: @unchecked Sendable {
    private let lock = NSLock()
    private var _events: [CcaEvent] = []
    var events: [CcaEvent] { lock.lock(); defer { lock.unlock() }; return _events }
    func add(_ e: CcaEvent) { lock.lock(); _events.append(e); lock.unlock() }
}

func consentEvent(_ item: String, _ scope: String, yes: Bool = true) -> CcaEvent {
    .consent(ConsentEvent(item: ConsentItem(rawValue: item), scope: scope, grant: scope.isEmpty ? item : "\(item):\(scope)",
                          title: "需要同意 \(item)", question: "继续吗？", detail: "", severity: .warn, defaultYes: yes))
}

func response(_ code: Int32, _ events: [CcaEvent] = [], stdout: String = "", stderr: String = "") -> CcaResponse {
    CcaResponse(exitCode: code, stdout: Data(stdout.utf8), stderr: Data(stderr.utf8), events: events)
}

let tinyUsage = #"{"version":1,"kind":"usage","generated_at":"2026-11-07T09:00:00Z","generated_epoch":1794042000.0,"accounts":[{"account":"a","key":"a@example.com","source":"none","error":"x","error_code":"port"}],"skipped_api_accounts":[]}"#
let tinyHello = #"{"version":1,"kind":"hello","api":1,"cca_version":"9.9.9","installed":true,"home":"/nonexistent/.cca","tools":{"claude":{"enabled":true,"bin":null},"codex":{"enabled":false,"bin":null}},"gui":{"app":"/Users/me/Applications/cca.app","app_version":null,"staged_version":null}}"#

final class TriggerSink: @unchecked Sendable {
    private let lock = NSLock()
    private var _items: [CcaWatcher.Trigger] = []
    var items: [CcaWatcher.Trigger] { lock.lock(); defer { lock.unlock() }; return _items }
    func add(_ t: CcaWatcher.Trigger) { lock.lock(); _items.append(t); lock.unlock() }
}

// MARK: AppModel（用演示数据和脚本化的 transport）

@MainActor final class HookLog {
    var opened: [URL] = []
    /// 打开时用的终端（bundle id），和 opened 一一对应。
    var terminals: [String] = []
    /// openInTerminal 回调的结果（模拟终端打不开）。
    var openSucceeds = true
    /// “选文件夹”面板要返回的结果。
    var pickedDirectory: String?
    var installed: [TerminalApp] = [.system]
    var urls: [URL] = []
    var copied: [String] = []
    var shown = 0
    var terminated = 0
    var announced: [String] = []
}

@MainActor func makeHooks(_ log: HookLog, _ prompter: Prompter) -> AppHooks {
    AppHooks(prompter: prompter,
             openInTerminal: { url, app, done in log.opened.append(url); log.terminals.append(app.id); done(log.openSucceeds) },
             installedTerminals: { log.installed }, chooseDirectory: { _ in log.pickedDirectory },
             openURL: { log.urls.append($0) },
             copy: { log.copied.append($0) }, showMainWindow: { log.shown += 1 }, terminate: { log.terminated += 1 },
             announce: { log.announced.append($0) })
}

/// 演示数据的 transport（不等待，事件立刻交出）；env 可以换文件（CCA_GUI_FIXTURE_APP 之类）。
func fixtureTransport(_ env: [String: String] = [:]) -> FixtureTransport {
    FixtureTransport(directory: fixtureDir, environment: env, stepDelay: 0)
}

/// 测试用的 AppModel：内存里的设置、记录用的 hooks，不找 cca、不碰 UserDefaults。
@MainActor func makeModel(_ transport: Transport, log: HookLog? = nil, prompter: Prompter = ScriptedPrompter([]),
                          appVersion: String = "0.0.0-test", prefs: MemoryPrefs = MemoryPrefs()) -> AppModel {
    AppModel(environment: [:], home: tmpRoot.path, appVersion: appVersion, defaults: prefs,
             hooks: makeHooks(log ?? HookLog(), prompter), transport: transport)
}

@MainActor func waitUntil(_ timeout: Double = 5, _ cond: () -> Bool) async -> Bool {
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        if cond() { return true }
        try? await Task.sleep(nanoseconds: 20_000_000)
    }
    return cond()
}
