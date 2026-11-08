import Foundation

/// 运行一个子进程（PROTOCOL 1）：
/// - stdin 接 /dev/null，自己一个会话（POSIX_SPAWN_SETSID），所以 cca 永远打不开 /dev/tty，一律按非交互处理；
/// - `run`：stdout / stderr 接管道、同时读（各最多 2 MB，多的读掉丢弃，免得子进程写满管道卡住）；超时或取消时给整个
///   进程组发 SIGTERM，3 秒后还没退出就 SIGKILL；主进程退出后再给它留下的子进程一点时间把管道里的东西写完（exitGrace）。
/// - `runToFiles`：stdout / stderr 接文件（app、web、router sync）：App 退出或崩溃时 cca 不会因为管道没人读被 SIGPIPE
///   杀掉；不限时、不能取消，一旦开始就让它跑完，结束后再读文件。
struct ProcessSpec: Sendable {
    var executable: String
    var arguments: [String]
    var environment: [String: String]
    var timeout: TimeInterval?
    var outputLimit: Int = 2 * 1024 * 1024
    var exitGrace: TimeInterval = 1.0
    var killGrace: TimeInterval = 3.0
    /// 后台的活（自动刷新用量、自动检查客户端更新）：utility 优先级，子进程也继承，不和用户正在做的事抢 CPU。
    var background = false

    init(executable: String, arguments: [String], environment: [String: String], timeout: TimeInterval?) {
        self.executable = executable
        self.arguments = arguments
        self.environment = environment
        self.timeout = timeout
    }
}

struct ProcessResult: Sendable {
    var pid: pid_t
    /// 正常退出时的退出码；被信号杀掉时是 128 + 信号。
    var exitCode: Int32
    var terminationSignal: Int32?
    var stdout: Data
    var stderr: Data
    var stdoutTruncated: Bool
    var stderrTruncated: Bool
    var timedOut: Bool
    var cancelled: Bool
    var duration: TimeInterval

    /// 宽松的 UTF-8：坏掉的字节换成 U+FFFD。
    var stdoutText: String { String(decoding: stdout, as: UTF8.self) }
    var stderrText: String { String(decoding: stderr, as: UTF8.self) }
}

enum ProcessError: Error, CustomStringConvertible, Equatable {
    case spawnFailed(path: String, errno: Int32)
    case pipeFailed(errno: Int32)

    var description: String {
        switch self {
        case .spawnFailed(let p, let e): return "运行 \(p) 失败：\(String(cString: strerror(e)))"
        case .pipeFailed(let e): return "建管道失败：\(String(cString: strerror(e)))"
        }
    }
}

/// 一个正在运行的进程的控制：超时、取消。线程安全。
final class ProcessControl: @unchecked Sendable {
    private let lock = NSLock()
    private var pid: pid_t = 0
    private var finished = false
    private var cancelRequested = false
    private var signalled = false
    private(set) var timedOut = false
    private(set) var cancelled = false
    let killGrace: TimeInterval

    init(killGrace: TimeInterval) {
        self.killGrace = killGrace
    }

    func attach(_ pid: pid_t) {
        lock.lock()
        self.pid = pid
        let cancelNow = cancelRequested
        lock.unlock()
        if cancelNow { terminate() }
    }

    func markFinished() {
        lock.lock()
        finished = true
        lock.unlock()
    }

    var isFinished: Bool {
        lock.lock()
        defer { lock.unlock() }
        return finished
    }

    func cancel() {
        lock.lock()
        cancelRequested = true
        if !finished { cancelled = true }
        lock.unlock()
        terminate()
    }

    func timeout() {
        lock.lock()
        if !finished { timedOut = true }
        lock.unlock()
        terminate()
    }

    /// SIGTERM 给整个进程组；killGrace 秒后还在就 SIGKILL。
    func terminate() {
        lock.lock()
        let p = pid
        let done = finished
        let already = signalled
        if p != 0 && !done { signalled = true }
        lock.unlock()
        guard p != 0, !done, !already else { return }
        if kill(-p, SIGTERM) != 0 { kill(p, SIGTERM) }
        DispatchQueue.global().asyncAfter(deadline: .now() + killGrace) { [self] in
            lock.lock()
            let stillRunning = !finished
            lock.unlock()
            if stillRunning {
                if kill(-p, SIGKILL) != 0 { kill(p, SIGKILL) }
            }
        }
    }
}

enum CcaProcess {
    /// 运行并等它结束。任务被取消时按上面的规则杀掉进程组，照样返回结果（cancelled 为真）。
    static func run(_ spec: ProcessSpec) async throws -> ProcessResult {
        let control = ProcessControl(killGrace: spec.killGrace)
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (cont: CheckedContinuation<ProcessResult, Error>) in
                do {
                    try start(spec, control: control) { cont.resume(returning: $0) }
                } catch {
                    cont.resume(throwing: error)
                }
            }
        } onCancel: {
            control.cancel()
        }
    }

    private static func makePipe() throws -> (read: Int32, write: Int32) {
        var fds: [Int32] = [0, 0]
        guard pipe(&fds) == 0 else { throw ProcessError.pipeFailed(errno: errno) }
        _ = fcntl(fds[0], F_SETFD, FD_CLOEXEC)
        _ = fcntl(fds[1], F_SETFD, FD_CLOEXEC)
        return (fds[0], fds[1])
    }

    /// 生成 argv / envp（C 字符串，调用方负责 free）。
    private static func cStrings(_ items: [String]) -> [UnsafeMutablePointer<CChar>?] {
        items.map { strdup($0) } + [nil]
    }

    private static func freeCStrings(_ p: [UnsafeMutablePointer<CChar>?]) {
        for x in p { free(x) }
    }

    /// 子进程的 stdout / stderr 接到哪里。
    enum Output {
        case pipe(Int32)
        case file(String)
        /// 和 stdout 同一个（只用于 stderr）
        case stdout
        case null
    }

    /// posix_spawn：自己一个会话、信号恢复默认、只继承 0/1/2；stdin 一律 /dev/null。
    private static func spawn(_ spec: ProcessSpec, stdout: Output, stderr: Output) throws -> pid_t {
        var attr: posix_spawnattr_t? = nil
        posix_spawnattr_init(&attr)
        defer { posix_spawnattr_destroy(&attr) }
        let flags = Int32(POSIX_SPAWN_SETSID | POSIX_SPAWN_CLOEXEC_DEFAULT | POSIX_SPAWN_SETSIGMASK | POSIX_SPAWN_SETSIGDEF)
        posix_spawnattr_setflags(&attr, Int16(truncatingIfNeeded: flags))
        var none = sigset_t()
        sigemptyset(&none)
        posix_spawnattr_setsigmask(&attr, &none)
        var all = sigset_t()
        sigfillset(&all)
        posix_spawnattr_setsigdefault(&attr, &all)
        if spec.background { posix_spawnattr_set_qos_class_np(&attr, QOS_CLASS_UTILITY) }

        var fa: posix_spawn_file_actions_t? = nil
        posix_spawn_file_actions_init(&fa)
        defer { posix_spawn_file_actions_destroy(&fa) }
        posix_spawn_file_actions_addopen(&fa, 0, "/dev/null", O_RDONLY, 0)
        for (fd, out) in [(Int32(1), stdout), (Int32(2), stderr)] {
            switch out {
            case .pipe(let w): posix_spawn_file_actions_adddup2(&fa, w, fd)
            case .file(let path): posix_spawn_file_actions_addopen(&fa, fd, path, O_WRONLY | O_CREAT | O_APPEND, 0o600)
            case .stdout: posix_spawn_file_actions_adddup2(&fa, 1, fd)
            case .null: posix_spawn_file_actions_addopen(&fa, fd, "/dev/null", O_WRONLY, 0)
            }
        }

        let argv = cStrings([spec.executable] + spec.arguments)
        let envp = cStrings(spec.environment.sorted { $0.key < $1.key }.map { "\($0.key)=\($0.value)" })
        defer {
            freeCStrings(argv)
            freeCStrings(envp)
        }
        var pid: pid_t = 0
        let rc = argv.withUnsafeBufferPointer { a in
            envp.withUnsafeBufferPointer { e in
                posix_spawn(&pid, spec.executable, &fa, &attr,
                            UnsafeMutablePointer(mutating: a.baseAddress!), UnsafeMutablePointer(mutating: e.baseAddress!))
            }
        }
        guard rc == 0 else { throw ProcessError.spawnFailed(path: spec.executable, errno: rc) }
        return pid
    }

    private static func start(_ spec: ProcessSpec, control: ProcessControl, done: @escaping @Sendable (ProcessResult) -> Void) throws {
        let out = try makePipe()
        let err: (read: Int32, write: Int32)
        do { err = try makePipe() } catch { close(out.read); close(out.write); throw error }
        let started = Date()
        let pid: pid_t
        do {
            pid = try spawn(spec, stdout: .pipe(out.write), stderr: .pipe(err.write))
        } catch {
            close(out.read); close(out.write); close(err.read); close(err.write)
            throw error
        }
        close(out.write)
        close(err.write)
        control.attach(pid)
        if let t = spec.timeout {
            DispatchQueue.global().asyncAfter(deadline: .now() + t) { control.timeout() }
        }
        let limit = spec.outputLimit, grace = spec.exitGrace
        let thread = Thread {
            var bufs = [Data(), Data()]
            var truncated = [false, false]
            var open = [true, true]
            let fds = [out.read, err.read]
            var exited = false
            var exitAt = Date()
            var status: Int32 = 0
            var chunk = [UInt8](repeating: 0, count: 65536)
            while true {
                if !exited {
                    let r = waitpid(pid, &status, WNOHANG)
                    if r == pid || (r == -1 && errno == ECHILD) {
                        exited = true
                        exitAt = Date()
                    }
                }
                if !open[0] && !open[1] { break }
                if exited && Date().timeIntervalSince(exitAt) >= grace { break }
                var pfds: [pollfd] = []
                var idx: [Int] = []
                for i in 0..<2 where open[i] {
                    pfds.append(pollfd(fd: fds[i], events: Int16(POLLIN), revents: 0))
                    idx.append(i)
                }
                let wait: Int32 = exited ? Int32(max(1, (grace - Date().timeIntervalSince(exitAt)) * 1000)) : 100
                let n = poll(&pfds, nfds_t(pfds.count), min(wait, 100))
                if n < 0 {
                    if errno == EINTR { continue }
                    break
                }
                if n == 0 { continue }
                for (j, p) in pfds.enumerated() where p.revents != 0 {
                    let i = idx[j]
                    let k = read(fds[i], &chunk, chunk.count)
                    if k > 0 {
                        let room = limit - bufs[i].count
                        if room > 0 {
                            bufs[i].append(contentsOf: chunk[0..<min(k, room)])
                        }
                        if k > room { truncated[i] = true }
                    } else if k == 0 || (errno != EINTR && errno != EAGAIN) {
                        open[i] = false
                    }
                }
            }
            if !exited {
                while waitpid(pid, &status, 0) == -1 && errno == EINTR {}
            }
            close(out.read)
            close(err.read)
            control.markFinished()
            let (exitCode, sig) = exitInfo(status)
            var code = exitCode
            if control.timedOut || control.cancelled, sig == nil, code == 0 {
                code = 128 + SIGTERM
            }
            done(ProcessResult(pid: pid, exitCode: code, terminationSignal: sig, stdout: bufs[0], stderr: bufs[1],
                               stdoutTruncated: truncated[0], stderrTruncated: truncated[1],
                               timedOut: control.timedOut, cancelled: control.cancelled,
                               duration: Date().timeIntervalSince(started)))
        }
        thread.name = "cca-process-\(pid)"
        thread.start()
    }

    /// waitpid 的 status → (退出码, 信号)；被信号杀掉时退出码是 128 + 信号。
    static func exitInfo(_ status: Int32) -> (Int32, Int32?) {
        let low = status & 0x7f
        return low == 0 ? ((status >> 8) & 0xff, nil) : (128 + low, low)
    }

    /// 输出接到文件、等它结束（app、web、router sync，PROTOCOL 1）：不限时、不能取消（App 退出时它照样跑完），
    /// 结束后读两个文件（各最多 outputLimit，留后面的部分），读完删掉。App 中途退出时文件留下，下次清理。
    static func runToFiles(_ spec: ProcessSpec, stdoutPath: String, stderrPath: String) async throws -> ProcessResult {
        let started = Date()
        let pid = try spawn(spec, stdout: .file(stdoutPath), stderr: .file(stderrPath))
        let limit = spec.outputLimit
        return await withCheckedContinuation { (cont: CheckedContinuation<ProcessResult, Never>) in
            let t = Thread {
                var status: Int32 = 0
                while waitpid(pid, &status, 0) == -1 && errno == EINTR {}
                let (code, sig) = exitInfo(status)
                let (out, outCut) = readTail(stdoutPath, limit: limit)
                let (err, errCut) = readTail(stderrPath, limit: limit)
                try? FileManager.default.removeItem(atPath: stdoutPath)
                try? FileManager.default.removeItem(atPath: stderrPath)
                cont.resume(returning: ProcessResult(pid: pid, exitCode: code, terminationSignal: sig, stdout: out, stderr: err,
                                                     stdoutTruncated: outCut, stderrTruncated: errCut, timedOut: false,
                                                     cancelled: false, duration: Date().timeIntervalSince(started)))
            }
            t.name = "cca-run-\(pid)"
            t.start()
        }
    }

    /// 文件的最后 limit 个字节（详情要的是最后几行）。
    static func readTail(_ path: String, limit: Int) -> (Data, Bool) {
        guard let h = FileHandle(forReadingAtPath: path) else { return (Data(), false) }
        defer { try? h.close() }
        let size = (try? h.seekToEnd()) ?? 0
        let start = size > UInt64(limit) ? size - UInt64(limit) : 0
        try? h.seek(toOffset: start)
        return ((try? h.readToEnd()) ?? Data(), start > 0)
    }

    /// 后台启动、不等它（`cca gui update --relaunch`：App 随后就退出）。stdout、stderr 都写进 logPath（没有就丢掉）。
    /// 进程在自己的会话里，App 退出不影响它；App 还开着时由后台线程回收，不留僵尸进程。
    @discardableResult
    static func spawnDetached(_ spec: ProcessSpec, logPath: String?) throws -> pid_t {
        try spawnMonitored(spec, logPath: logPath) { _ in }
    }

    /// 后台启动，退出时（在后台线程上）把退出码交给 onExit（内置代理的内核）。stdout、stderr 都追加进 logPath（没有就丢掉）。
    /// 自己一个会话：要停的时候给整个进程组发信号（CoreProcessHandle）。
    @discardableResult
    static func spawnMonitored(_ spec: ProcessSpec, logPath: String?, onExit: @escaping @Sendable (Int32) -> Void) throws -> pid_t {
        let pid = try spawn(spec, stdout: logPath.map { .file($0) } ?? .null, stderr: logPath == nil ? .null : .stdout)
        let t = Thread {
            var status: Int32 = 0
            while waitpid(pid, &status, 0) == -1 && errno == EINTR {}
            onExit(exitInfo(status).0)
        }
        t.name = "cca-bg-\(pid)"
        t.start()
        return pid
    }
}

/// 一个在后台跑着的进程（内置代理的内核）：停掉它时 SIGTERM 整个进程组，killGrace 秒后还在就 SIGKILL。
/// 已经退出了就不再发信号（pid 可能已经给了别的进程）。线程安全。
final class CoreProcessHandle: CoreHandle, @unchecked Sendable {
    private let lock = NSLock()
    private var pid: pid_t = 0
    private var exited = false
    let killGrace: TimeInterval

    init(killGrace: TimeInterval = 3) {
        self.killGrace = killGrace
    }

    func attach(_ pid: pid_t) {
        lock.withLock { self.pid = pid }
    }

    func markExited() {
        lock.withLock { exited = true }
    }

    private var running: pid_t? {
        lock.withLock { pid != 0 && !exited ? pid : nil }
    }

    /// wait：在这里等它退出（最多 killGrace 秒，到时 SIGKILL）；App 退出前用，免得来不及发 SIGKILL。
    func terminate(wait: Bool) {
        guard let p = running else { return }
        if kill(-p, SIGTERM) != 0 { kill(p, SIGTERM) }
        let kill9: @Sendable () -> Void = { [self] in
            if let p = running, kill(-p, SIGKILL) != 0 { kill(p, SIGKILL) }
        }
        if wait {
            let deadline = Date().addingTimeInterval(killGrace)
            while running != nil && kill(p, 0) == 0 && Date() < deadline { usleep(50_000) }
            kill9()
        } else {
            DispatchQueue.global().asyncAfter(deadline: .now() + killGrace, execute: kill9)
        }
    }
}

extension String {
    /// 最后几行（给“详情”用）。
    func lastLines(_ n: Int) -> String {
        let lines = split(separator: "\n", omittingEmptySubsequences: false)
        var tail = lines.suffix(n + 1)
        while let l = tail.last, l.trimmingCharacters(in: .whitespaces).isEmpty { tail.removeLast() }
        return tail.suffix(n).joined(separator: "\n")
    }
}
