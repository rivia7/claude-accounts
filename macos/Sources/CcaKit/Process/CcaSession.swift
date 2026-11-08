import Foundation

/// 提问会话（PROTOCOL 4.6）运行时发生的事，按顺序交出。
enum SessionUpdate: Sendable {
    case event(CcaEvent)
    /// cca 最新写到 stderr 的一行（给人看的进度，原样显示，不解析）。
    case output(String)
    case finished(CcaResponse)
}

/// 一个边问边答的 cca 进程。
protocol CcaSession: AnyObject, Sendable {
    var updates: AsyncStream<SessionUpdate> { get }
    /// 回答当前这一页（每个字段一行，已经按协议拼好）。可能含密钥：不记日志。
    func answer(_ lines: [String])
    /// 关掉答案管道：cca 在下一个提问处按取消退出（绝不当成回车取默认值）。
    func closeAnswers()
    /// 给整个进程组发 SIGTERM，3 秒后还在就 SIGKILL（步骤卡住时用）。
    func stop()
}

struct SessionUnsupported: Error, CustomStringConvertible {
    var description: String { "这里不能运行提问会话" }
}

final class ProcessSession: CcaSession, @unchecked Sendable {
    let updates: AsyncStream<SessionUpdate>
    private let continuation: AsyncStream<SessionUpdate>.Continuation
    /// 答案管道的写端（只在 writer 这个串行队列上用）。
    private var answers: Int32
    private let writer = DispatchQueue(label: "cca.session.answers")
    private let control: ProcessControl
    private let pid: pid_t

    init(spec: ProcessSpec, events: URL, stdout: URL, stderr: URL) throws {
        var c: AsyncStream<SessionUpdate>.Continuation!
        updates = AsyncStream(bufferingPolicy: .unbounded) { c = $0 }
        continuation = c
        let started = Date()
        let spawned = try CcaProcess.spawnSession(spec, stdoutPath: stdout.path, stderrPath: stderr.path)
        pid = spawned.pid
        answers = spawned.answers
        control = ProcessControl(killGrace: spec.killGrace)
        control.attach(pid)
        let limit = spec.outputLimit
        let thread = Thread { [self] in
            watch(events: events, stdout: stdout, stderr: stderr, limit: limit, started: started)
        }
        thread.name = "cca-session-\(pid)"
        thread.start()
    }

    private func watch(events: URL, stdout: URL, stderr: URL, limit: Int, started: Date) {
        let tail = EventTail(url: events)
        var status: Int32 = 0
        var done = false
        var line = ""
        while true {
            if !done {
                let r = waitpid(pid, &status, WNOHANG)
                done = r == pid || (r == -1 && errno == ECHILD)
            }
            for e in tail.poll() { continuation.yield(.event(e)) }
            if let l = CcaProcess.lastLine(stderr.path), l != line {
                line = l
                continuation.yield(.output(l))
            }
            if done { break }
            Thread.sleep(forTimeInterval: 0.1)
        }
        control.markFinished()
        closeAnswers()
        let (code, _) = CcaProcess.exitInfo(status)
        let (out, _) = CcaProcess.readTail(stdout.path, limit: limit)
        let (err, _) = CcaProcess.readTail(stderr.path, limit: limit)
        let all = tail.all()
        for u in [stdout, stderr, events] { try? FileManager.default.removeItem(at: u) }
        continuation.yield(.finished(CcaResponse(exitCode: code, stdout: out, stderr: err, events: all,
                                                 duration: Date().timeIntervalSince(started))))
        continuation.finish()
    }

    func answer(_ lines: [String]) {
        var data = Data(lines.joined(separator: "\n").utf8)
        data.append(0x0A)
        writer.async { [self] in
            if answers >= 0 { CcaProcess.writeAll(answers, data) }
            data.resetBytes(in: 0..<data.count)
        }
    }

    func closeAnswers() {
        writer.async { [self] in
            if answers >= 0 { close(answers) }
            answers = -1
        }
    }

    func stop() {
        control.terminate()
    }
}
