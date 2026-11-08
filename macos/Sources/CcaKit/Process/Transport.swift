import Foundation

/// 一次 cca 调用：`cca` 后面的参数、超时、要不要事件文件、授权。
struct CcaRequest: Sendable, Hashable {
    var arguments: [String]
    var timeout: TimeInterval
    var wantsEvents: Bool
    var grants: [String]
    var extraEnvironment: [String: String]
    /// 一旦开始就要跑完（app、web、router sync，PROTOCOL 1）：输出接文件不接管道，不限时、不取消，App 退出时也不杀。
    var runsToCompletion: Bool
    /// 后台的活：低优先级运行（ProcessSpec.background）。
    var background: Bool

    init(arguments: [String], timeout: TimeInterval, wantsEvents: Bool = false, grants: [String] = [],
         extraEnvironment: [String: String] = [:], runsToCompletion: Bool = false, background: Bool = false) {
        self.arguments = arguments
        self.timeout = timeout
        self.wantsEvents = wantsEvents
        self.grants = grants
        self.extraEnvironment = extraEnvironment
        self.runsToCompletion = runsToCompletion
        self.background = background
    }
}

struct CcaResponse: Sendable {
    var exitCode: Int32
    var stdout: Data
    var stderr: Data
    var events: [CcaEvent]
    var timedOut: Bool = false
    var cancelled: Bool = false
    var duration: TimeInterval = 0

    var stdoutText: String { String(decoding: stdout, as: UTF8.self) }
    var stderrText: String { String(decoding: stderr, as: UTF8.self) }
}

/// 怎么运行 cca：真的进程（ProcessTransport），或者演示 / 测试用的固定数据（FixtureTransport）。
protocol Transport: Sendable {
    /// 演示模式：不运行 cca。
    var isFixture: Bool { get }
    /// 运行并等它结束；运行期间新写进事件文件的事件经 onEvent 依次交出。
    func send(_ request: CcaRequest, onEvent: (@Sendable (CcaEvent) -> Void)?) async throws -> CcaResponse
    /// 后台启动、不等它（`gui update --relaunch`）。
    func spawnDetached(_ request: CcaRequest) throws
    /// 内置代理的内核（PROTOCOL 7）：后台运行 `cca _core run`，不限时，输出追加进 logPath；退出时（后台线程上）调 onExit。
    func startCore(logPath: String, onExit: @escaping @Sendable (Int32) -> Void) throws -> CoreHandle
}

/// 一个在跑的内置代理内核：停掉它（wait：等它退出再返回，App 退出前用）。
protocol CoreHandle: AnyObject, Sendable {
    func terminate(wait: Bool)
}

struct CoreUnsupported: Error, CustomStringConvertible {
    var description: String { "演示数据不运行内置代理" }
}

extension Transport {
    /// 演示数据、测试替身：不运行内核。
    func startCore(logPath: String, onExit: @escaping @Sendable (Int32) -> Void) throws -> CoreHandle {
        throw CoreUnsupported()
    }
}

struct ProcessTransport: Transport {
    let ccaPath: String
    let baseEnvironment: [String: String]
    let home: String
    let eventsDirectory: URL
    /// 跑完才读的输出（app、web、router sync）和 `gui update --relaunch` 的日志。
    let runsDirectory: URL

    var isFixture: Bool { false }

    init(ccaPath: String, baseEnvironment: [String: String] = ProcessInfo.processInfo.environment,
         home: String = NSHomeDirectory(), eventsDirectory: URL? = nil, runsDirectory: URL? = nil) {
        self.ccaPath = ccaPath
        self.baseEnvironment = baseEnvironment
        self.home = home
        self.eventsDirectory = eventsDirectory
            ?? FileManager.default.temporaryDirectory.appendingPathComponent("cca-gui-events", isDirectory: true)
        self.runsDirectory = runsDirectory ?? ProcessTransport.defaultRunsDirectory()
    }

    /// `~/Library/Caches/local.cca.app/runs/`
    static func defaultRunsDirectory() -> URL {
        TerminalHandoff.defaultDirectory().deletingLastPathComponent().appendingPathComponent("runs", isDirectory: true)
    }

    private static func prepare(_ dir: URL) {
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        TerminalHandoff.cleanup(directory: dir, olderThan: 86400, extensions: ["jsonl", "out", "err", "log"])
    }

    /// 新的空文件（600）。App 中途退出时 cca 还在往里写的文件会留下：一天前的在下次建文件时清掉。
    private func newFile(in dir: URL, _ name: String) -> URL? {
        ProcessTransport.prepare(dir)
        let url = dir.appendingPathComponent(name)
        guard FileManager.default.createFile(atPath: url.path, contents: Data(), attributes: [.posixPermissions: 0o600]) else { return nil }
        return url
    }

    func send(_ request: CcaRequest, onEvent: (@Sendable (CcaEvent) -> Void)?) async throws -> CcaResponse {
        let eventsURL = request.wantsEvents ? newFile(in: eventsDirectory, "events-\(UUID().uuidString).jsonl") : nil
        defer { if let eventsURL { try? FileManager.default.removeItem(at: eventsURL) } }
        let env = CcaEnvironment.make(base: baseEnvironment, home: home, eventsPath: eventsURL?.path,
                                      grants: request.grants, extra: request.extraEnvironment)
        var spec = ProcessSpec(executable: ccaPath, arguments: request.arguments, environment: env, timeout: request.timeout)
        spec.background = request.background
        let tail = eventsURL.map { EventTail(url: $0) }
        var poller: Task<Void, Never>? = nil
        if let tail, let onEvent {
            poller = Task.detached {
                while !Task.isCancelled {
                    try? await Task.sleep(nanoseconds: 150_000_000)
                    for e in tail.poll() { onEvent(e) }
                }
            }
        }
        let result: ProcessResult
        do {
            if request.runsToCompletion {
                let id = UUID().uuidString
                guard let out = newFile(in: runsDirectory, "run-\(id).out"), let err = newFile(in: runsDirectory, "run-\(id).err") else {
                    throw ProcessError.spawnFailed(path: runsDirectory.path, errno: EACCES)
                }
                result = try await CcaProcess.runToFiles(spec, stdoutPath: out.path, stderrPath: err.path)
            } else {
                result = try await CcaProcess.run(spec)
            }
        } catch {
            poller?.cancel()
            throw error
        }
        if let poller {
            poller.cancel()
            await poller.value
        }
        if let tail, let onEvent {
            for e in tail.poll() { onEvent(e) }
        }
        return CcaResponse(exitCode: result.exitCode, stdout: result.stdout, stderr: result.stderr, events: tail?.all() ?? [],
                           timedOut: result.timedOut, cancelled: result.cancelled, duration: result.duration)
    }

    /// 不设 CCA_EVENTS、不限时，输出接到 runs 目录里的一个文件（PROTOCOL 5）。
    func spawnDetached(_ request: CcaRequest) throws {
        let env = CcaEnvironment.make(base: baseEnvironment, home: home, grants: request.grants, extra: request.extraEnvironment)
        let spec = ProcessSpec(executable: ccaPath, arguments: request.arguments, environment: env, timeout: nil)
        let log = newFile(in: runsDirectory, "detached-\(UUID().uuidString).log")
        try CcaProcess.spawnDetached(spec, logPath: log?.path)
    }

    /// `cca _core run`（PROTOCOL 7）：cca 生成配置后 exec 内核，这个进程就是内核本身。不设 CCA_EVENTS，不限时。
    func startCore(logPath: String, onExit: @escaping @Sendable (Int32) -> Void) throws -> CoreHandle {
        let env = CcaEnvironment.make(base: baseEnvironment, home: home)
        let spec = ProcessSpec(executable: ccaPath, arguments: ["_core", "run"], environment: env, timeout: nil)
        let handle = CoreProcessHandle()
        let pid = try CcaProcess.spawnMonitored(spec, logPath: logPath) { code in
            handle.markExited()
            onExit(code)
        }
        handle.attach(pid)
        return handle
    }

    /// 内置代理内核的日志：`~/Library/Caches/local.cca.app/core.log`（不放 runs 目录：那里一天前的文件会被清掉）
    static func coreLogPath() -> String {
        defaultRunsDirectory().deletingLastPathComponent().appendingPathComponent("core.log").path
    }
}
