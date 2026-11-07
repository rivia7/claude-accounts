import Foundation

/// 内置代理的内核（PROTOCOL 7）：state 里本机客户端是内置代理（`config.local.client == builtin`）时，App 运行 `cca _core run`
/// （cca 照账号表生成配置后 exec mihomo），让它一直跑着：
/// - 退出了就隔一会儿重启：1、2、4…秒，最多 60 秒；跑够 60 秒才退出的，从 1 秒重新算。
/// - 不再需要（换了客户端、关掉了）时停掉；App 退出前停掉并等它退出（SIGTERM 进程组，3 秒后 SIGKILL）。
/// - 输出追加进日志（`~/Library/Caches/local.cca.app/core.log`，每次启动前超过 1 MB 就清空）；退出了的话，最后几行给界面显示。
/// 配置、端口、节点都是 cca 的事：这里只管进程活着。
@MainActor
final class CoreHost {
    enum Status: Equatable, Sendable {
        case off
        case running(since: Date)
        /// 退出了（或者没启动起来），retryAt 时再试；detail 是日志的最后几行
        case waiting(retryAt: Date, exitCode: Int32, detail: String)
    }

    typealias Starter = @MainActor (_ logPath: String, _ onExit: @escaping @Sendable (Int32) -> Void) throws -> CoreHandle

    private(set) var status: Status = .off {
        didSet { if status != oldValue { onChange?(status) } }
    }
    var onChange: ((Status) -> Void)?
    let logPath: String
    private let start: Starter
    private let firstBackoff: TimeInterval
    private let maxBackoff: TimeInterval
    private let resetAfter: TimeInterval
    private var wanted = false
    private var handle: CoreHandle?
    private var startedAt = Date()
    private var backoff: TimeInterval
    private var retry: Task<Void, Never>?
    /// 每次启动、停止加一：旧进程迟到的退出通知、过期的重试都按它认出来丢掉
    private var generation = 0

    init(logPath: String, firstBackoff: TimeInterval = 1, maxBackoff: TimeInterval = 60, resetAfter: TimeInterval = 60,
         start: @escaping Starter) {
        self.logPath = logPath
        self.firstBackoff = firstBackoff
        self.maxBackoff = maxBackoff
        self.resetAfter = resetAfter
        self.backoff = firstBackoff
        self.start = start
    }

    var isWanted: Bool { wanted }

    /// state 变了：要不要内核在跑。
    func setWanted(_ w: Bool) {
        guard w != wanted else { return }
        wanted = w
        if w {
            backoff = firstBackoff
            launch()
        } else {
            halt(wait: false)
        }
    }

    /// 界面上点了“现在重启”：不等退避，马上再起一次。
    func retryNow() {
        guard wanted, handle == nil else { return }
        backoff = firstBackoff
        launch()
    }

    /// App 退出前：停掉并等它退出。
    func shutdown() {
        wanted = false
        halt(wait: true)
    }

    private func halt(wait: Bool) {
        retry?.cancel()
        retry = nil
        generation += 1
        handle?.terminate(wait: wait)
        handle = nil
        status = .off
    }

    private func launch() {
        retry?.cancel()
        retry = nil
        guard wanted, handle == nil else { return }
        generation += 1
        let gen = generation
        trimLog()
        do {
            handle = try start(logPath) { [weak self] code in
                Task { @MainActor in self?.exited(gen: gen, code: code) }
            }
            startedAt = Date()
            status = .running(since: startedAt)
        } catch {
            handle = nil
            schedule(code: -1, detail: String(describing: error))
        }
    }

    private func exited(gen: Int, code: Int32) {
        guard gen == generation else { return }   // 已经停掉、换掉的那个
        handle = nil
        guard wanted else {
            status = .off
            return
        }
        if Date().timeIntervalSince(startedAt) >= resetAfter { backoff = firstBackoff }
        schedule(code: code, detail: tail())
    }

    private func schedule(code: Int32, detail: String) {
        let wait = backoff
        backoff = min(backoff * 2, maxBackoff)
        status = .waiting(retryAt: Date().addingTimeInterval(wait), exitCode: code, detail: detail)
        let gen = generation
        retry = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(wait * 1_000_000_000))
            guard !Task.isCancelled, let self, self.generation == gen else { return }
            self.launch()
        }
    }

    /// 日志超过 1 MB 就清空（内核的日志级别是 warning，平时几乎不写）。
    private func trimLog() {
        let fm = FileManager.default
        try? fm.createDirectory(atPath: (logPath as NSString).deletingLastPathComponent, withIntermediateDirectories: true,
                                attributes: [.posixPermissions: 0o700])
        if let size = (try? fm.attributesOfItem(atPath: logPath))?[.size] as? NSNumber, size.intValue > 1_048_576 {
            try? fm.removeItem(atPath: logPath)
        }
    }

    /// 日志的最后几行（去掉空行）。
    private func tail() -> String {
        let (data, _) = CcaProcess.readTail(logPath, limit: 4096)
        return String(decoding: data, as: UTF8.self).lastLines(4)
    }
}
