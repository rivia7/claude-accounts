import Foundation
import Observation

/// 填到一半的密文：不放进被观察的状态，交给 cca 后马上清掉；打印出来也看不到内容。
final class SecretDrafts: CustomStringConvertible, CustomDebugStringConvertible {
    private var values: [String: String] = [:]

    subscript(id: String) -> String {
        get { values[id] ?? "" }
        set { values[id] = newValue }
    }

    func removeAll() { values.removeAll() }

    var description: String { "<密文>" }
    var debugDescription: String { "<密文>" }
}

/// 一条在 App 里问完的流程（提问会话，PROTOCOL 4.6）：cca 问一页，这里显示一页，答案交回去；问什么、怎么校验、下一步是什么都由 cca 决定。
@MainActor @Observable
final class FlowModel: Identifiable {
    enum Phase: Equatable {
        /// 等 cca（刚开始、答完一页之后）。
        case working
        case asking(AskEvent)
        /// 做完了，但有要提醒的（例如账号加好了，路由器没同步成）。
        case warned(message: String, warnings: [String])
        case failed(Failure)
    }

    enum End: Equatable {
        /// 退出码 0。没有 result 事件表示用户选了不做（比如删除时答“否”）。
        case success(ResultEvent?)
        case cancelled
        /// 这一步要在终端里做（App 弹终端交接）。
        case terminal(TerminalEvent)
    }

    nonisolated let id = UUID()
    let handoff: Handoff
    private(set) var phase: Phase = .working
    /// cca 最新的一行输出（等待时显示）。
    private(set) var output = ""
    /// 开始等 cca 的时间（等太久时给“停止”按钮）。
    private(set) var workingSince = Date()
    /// 当前这一页的草稿（字段 id → 值），密文不在这里。
    var draft: [String: String] = [:]
    private(set) var filledSecrets: Set<String> = []

    @ObservationIgnored private let secrets = SecretDrafts()
    @ObservationIgnored private let session: CcaSession
    @ObservationIgnored private let onEnd: @MainActor (FlowModel, End) -> Void
    /// cca 说做完了什么（“做完了，要提醒”的页面关掉时，App 据此选中新账号）。
    @ObservationIgnored private(set) var result: ResultEvent?
    @ObservationIgnored private var fails: [FailEvent] = []
    @ObservationIgnored private var terminal: TerminalEvent?
    @ObservationIgnored private var cancelled = false

    init(handoff: Handoff, session: CcaSession, onEnd: @escaping @MainActor (FlowModel, End) -> Void) {
        self.handoff = handoff
        self.session = session
        self.onEnd = onEnd
        let updates = session.updates
        Task { [self] in
            for await u in updates { handle(u) }
        }
    }

    private func handle(_ u: SessionUpdate) {
        switch u {
        case .event(.ask(let e)):
            if cancelled { return }
            guard e.isDisplayable else {
                // 这个版本的 App 显示不了：不能漏掉问题，取消这次，退回终端（失败页上有按钮）
                session.closeAnswers()
                unreadable = true
                return
            }
            begin(e)
        case .event(.result(let r)): result = r
        case .event(.fail(let f)): fails.append(f)
        case .event(.terminal(let t)): terminal = t
        case .event: break
        case .output(let line): output = line
        case .finished(let resp): finished(resp)
        }
    }

    @ObservationIgnored private var unreadable = false

    private func begin(_ e: AskEvent) {
        var d: [String: String] = [:]
        for f in e.fields where f.kind != .secret && f.kind != .confirm && f.kind != .code {
            d[f.id] = f.value.isEmpty ? f.defaultValue : f.value
        }
        draft = d
        secrets.removeAll()
        filledSecrets = []
        phase = .asking(e)
    }

    private func finished(_ resp: CcaResponse) {
        if cancelled { return }
        if resp.exitCode == 0 {
            if fails.isEmpty {
                onEnd(self, .success(result))
            } else {
                phase = .warned(message: nonEmpty(result?.message) ?? "已完成", warnings: fails.map(\.message))
            }
            return
        }
        if !unreadable, let t = terminal {
            onEnd(self, .terminal(t))
            return
        }
        if !unreadable, fails.last?.code == .cancelled {
            onEnd(self, .cancelled)
            return
        }
        let message = unreadable ? "这个版本的 App 显示不了这一步，可以在终端里完成，或者更新 App。"
            : nonEmpty(fails.last?.message) ?? "cca 没有完成（退出码 \(resp.exitCode)）"
        phase = .failed(Failure(reason: .command, code: unreadable ? nil : fails.last?.code, message: message,
                                stderrTail: resp.stderrText.lastLines(12), exitCode: resp.exitCode, tool: nil, key: nil))
    }

    // MARK: 答题

    func secret(_ id: String) -> String { secrets[id] }

    func setSecret(_ id: String, _ value: String) {
        secrets[id] = value
        if value.isEmpty { filledSecrets.remove(id) } else { filledSecrets.insert(id) }
    }

    /// 多选题选中了哪些（答案是空格隔开的 value）。
    func selected(_ id: String) -> Set<String> {
        Set((draft[id] ?? "").split(separator: " ").map(String.init))
    }

    func toggle(_ id: String, _ value: String) {
        var s = selected(id)
        if s.contains(value) { s.remove(value) } else { s.insert(value) }
        draft[id] = s.sorted { (Int($0) ?? 0) < (Int($1) ?? 0) }.joined(separator: " ")
    }

    /// 只拦空着的必填项，别的判断都交给 cca。
    var canSubmit: Bool {
        guard case .asking(let e) = phase else { return false }
        return e.fields.allSatisfy { f in
            switch f.kind {
            case .secret: return f.optional || filledSecrets.contains(f.id)
            case .multi: return selected(f.id).count >= max(f.min ?? 1, f.optional ? 0 : 1)
            case .confirm, .code: return true
            default: return f.optional || !(draft[f.id] ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            }
        }
    }

    func submit() {
        guard canSubmit, case .asking(let e) = phase else { return }
        send(e, e.fields.filter { $0.kind != .code }.map { f in f.kind == .secret ? secrets[f.id] : draft[f.id] ?? "" })
    }

    /// 回答“是 / 否”的一页。
    func confirm(_ yes: Bool) {
        guard case .asking(let e) = phase, e.confirmField != nil else { return }
        send(e, [yes ? "y" : "n"])
    }

    private func send(_ e: AskEvent, _ values: [String]) {
        let lines = zip(e.fields.filter { $0.kind != .code }, values).map { f, v in "\(e.seq) \(f.id) \(FlowModel.oneLine(v, f.kind))" }
        session.answer(lines)
        secrets.removeAll()
        filledSecrets = []
        draft = [:]
        workingSince = Date()
        phase = .working
    }

    /// 答案是一行一个字段：换行不能混进去。文字去掉前后的空白，密文原样（中间的换行去掉）。
    nonisolated static func oneLine(_ value: String, _ kind: PromptKind) -> String {
        let s = value.replacingOccurrences(of: "\r", with: "").replacingOccurrences(of: "\n", with: kind == .secret ? "" : " ")
        return kind == .secret ? s : s.trimmingCharacters(in: .whitespaces)
    }

    /// 用户取消（问题挂着的时候）：关掉答案管道，cca 自己退出，什么都不再问。
    func cancel() {
        guard !cancelled else { return }
        cancelled = true
        session.closeAnswers()
        secrets.removeAll()
        onEnd(self, .cancelled)
    }

    /// 步骤卡住了：停掉进程（结果按失败显示）。
    func stop() {
        session.stop()
    }
}

extension FlowModel: Equatable {
    nonisolated static func == (a: FlowModel, b: FlowModel) -> Bool { a === b }
}
