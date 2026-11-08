import Foundation

/// `CCA_EVENTS` 文件里的一行（PROTOCOL 4.1）。不认识的事件类型收成 `.other`，不报错。
enum CcaEvent: Sendable, Equatable {
    case step(StepEvent)
    case consent(ConsentEvent)
    case terminal(TerminalEvent)
    case fail(FailEvent)
    case result(ResultEvent)
    /// 提问会话（PROTOCOL 4.6）：cca 在等这一页的答案。
    case ask(AskEvent)
    case other(String)

    var type: EventType {
        switch self {
        case .step: return .step
        case .consent: return .consent
        case .terminal: return .terminal
        case .fail: return .fail
        case .result: return .result
        case .ask: return .ask
        case .other(let t): return EventType(rawValue: t)
        }
    }

    var consent: ConsentEvent? { if case .consent(let e) = self { return e }; return nil }
    var terminal: TerminalEvent? { if case .terminal(let e) = self { return e }; return nil }
    var fail: FailEvent? { if case .fail(let e) = self { return e }; return nil }
    var result: ResultEvent? { if case .result(let e) = self { return e }; return nil }
    var step: StepEvent? { if case .step(let e) = self { return e }; return nil }
    var ask: AskEvent? { if case .ask(let e) = self { return e }; return nil }
}

struct StepEvent: Sendable, Equatable {
    var code: StepCode
}

/// 要用户同意的一件事。对话框只用这里的 title / question / detail / severity / default；`grant` 原样回传。
struct ConsentEvent: Sendable, Equatable, Hashable {
    var item: ConsentItem
    var scope: String
    var grant: String
    var title: String
    var question: String
    var detail: String
    var severity: Severity
    /// `default` 为 "yes" 时默认按钮是同意；其它值（包括不认识的）默认不同意。
    var defaultYes: Bool
}

struct TerminalEvent: Sendable, Equatable, Hashable {
    var code: TerminalCode
    /// `cca` 后面的参数（Codex 账号带 `codex`）。
    var argv: [String]
    /// cca 给的、能直接复制进终端的完整命令；没给时由 Handoff 按 argv 拼（displayCommand）。
    var display: String?
    var message: String
}

struct FailEvent: Sendable, Equatable {
    var code: FailCode
    var message: String
}

struct ResultEvent: Sendable, Equatable {
    var code: ResultCode
    var route: Route?
    var node: String?
    var port: Int?
    var message: String?
    /// 提问会话：做完之后选中哪个账号。
    var tool: ToolID? = nil
    var key: String? = nil
}

private struct RawEvent: Decodable {
    var v: Int?
    var ev: String?
    var code: String?
    var item: ConsentItem?
    var scope: String?
    var grant: String?
    var title: String?
    var question: String?
    var detail: String?
    var severity: Severity?
    var defaultValue: String?
    var argv: [String]?
    var display: String?
    var message: String?
    var route: Route?
    var node: String?
    var port: Int?
    var seq: Int?
    var q: String?
    var fields: [PromptField]?
    var tool: ToolID?
    var key: String?

    enum CodingKeys: String, CodingKey {
        case v, ev, code, item, scope, grant, title, question, detail, severity
        case defaultValue = "default"
        case argv, display, message, route, node, port, seq, q, fields, tool, key
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        v = c.int(.v)
        ev = c.string(.ev)
        code = c.stringish(.code)
        item = c.lossy(ConsentItem.self, .item)
        scope = c.stringish(.scope)
        grant = c.string(.grant)
        title = c.string(.title)
        question = c.string(.question)
        detail = c.string(.detail)
        severity = c.lossy(Severity.self, .severity)
        defaultValue = c.stringish(.defaultValue)
        argv = c.lossyArray(String.self, .argv)
        display = c.string(.display)
        message = c.string(.message)
        route = c.lossy(Route.self, .route)
        node = c.string(.node)
        port = c.int(.port)
        seq = c.int(.seq)
        q = c.string(.q)
        fields = c.lossyArray(PromptField.self, .fields)
        tool = c.lossy(ToolID.self, .tool)
        key = c.string(.key)
    }
}

enum EventLog {
    /// 解析整份事件文件：一行一个 JSON；坏掉的行跳过；最后一行没有换行也算。
    static func parse(_ data: Data) -> [CcaEvent] {
        data.split(separator: 0x0A, omittingEmptySubsequences: true).compactMap { parseLine(Data($0)) }
    }

    static func parseLine(_ line: Data) -> CcaEvent? {
        let t = DocumentDecoder.trim(line)
        guard t.first == UInt8(ascii: "{"), let raw = try? JSONDecoder().decode(RawEvent.self, from: t) else { return nil }
        guard let ev = raw.ev, !ev.isEmpty else { return nil }
        switch EventType(rawValue: ev) {
        case .step:
            guard let code = raw.code else { return .other(ev) }
            return .step(StepEvent(code: StepCode(rawValue: code)))
        case .consent:
            // 没有 grant 就没法授权（App 不自己拼），当作不认识的事件
            guard let grant = raw.grant, !grant.isEmpty else { return .other(ev) }
            return .consent(ConsentEvent(item: raw.item ?? ConsentItem(rawValue: ""), scope: raw.scope ?? "", grant: grant,
                                         title: raw.title ?? "", question: raw.question ?? "", detail: raw.detail ?? "",
                                         severity: raw.severity ?? .info, defaultYes: raw.defaultValue == "yes"))
        case .terminal:
            guard let argv = raw.argv, !argv.isEmpty else { return .other(ev) }
            return .terminal(TerminalEvent(code: TerminalCode(rawValue: raw.code ?? ""), argv: argv, display: nonEmpty(raw.display),
                                           message: raw.message ?? ""))
        case .fail:
            return .fail(FailEvent(code: FailCode(rawValue: raw.code ?? "error"), message: raw.message ?? ""))
        case .result:
            return .result(ResultEvent(code: ResultCode(rawValue: raw.code ?? ""), route: raw.route, node: raw.node,
                                       port: raw.port, message: raw.message, tool: raw.tool, key: raw.key))
        case .ask:
            guard let seq = raw.seq else { return .other(ev) }
            return .ask(AskEvent(seq: seq, q: raw.q ?? "", title: raw.title ?? "", message: raw.message ?? "", fields: raw.fields ?? []))
        default:
            return .other(ev)
        }
    }
}

/// 一边运行一边读事件文件（cca 往里追加），按完整的行交出新事件；最后再整份读一遍。
final class EventTail: @unchecked Sendable {
    let url: URL
    private var offset: UInt64 = 0
    private var partial = Data()
    private let lock = NSLock()

    init(url: URL) {
        self.url = url
    }

    /// 读到目前为止新追加的完整行。
    func poll() -> [CcaEvent] {
        lock.lock()
        defer { lock.unlock() }
        guard let h = try? FileHandle(forReadingFrom: url) else { return [] }
        defer { try? h.close() }
        do {
            try h.seek(toOffset: offset)
            let chunk = try h.readToEnd() ?? Data()
            offset += UInt64(chunk.count)
            partial.append(chunk)
        } catch {
            return []
        }
        guard let lastNL = partial.lastIndex(of: 0x0A) else { return [] }
        let complete = partial[partial.startIndex...lastNL]
        partial = Data(partial[partial.index(after: lastNL)...])
        return EventLog.parse(Data(complete))
    }

    /// 整份文件的全部事件（命令结束后用：以它为准）。
    func all() -> [CcaEvent] {
        guard let d = try? Data(contentsOf: url) else { return [] }
        return EventLog.parse(d)
    }
}
