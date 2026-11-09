import Foundation

/// 提问会话（PROTOCOL 4.6）：一页里的字段用什么控件显示。
struct PromptKind: KnownValues {
    let rawValue: String
    init(rawValue: String) { self.rawValue = rawValue }
    static let text = PromptKind(rawValue: "text")
    static let secret = PromptKind(rawValue: "secret")
    static let choice = PromptKind(rawValue: "choice")
    static let multi = PromptKind(rawValue: "multi")
    static let confirm = PromptKind(rawValue: "confirm")
    /// 只显示、不用回答的一段多行文字（`value`），带“复制”。
    static let code = PromptKind(rawValue: "code")
    static let allKnown: [PromptKind] = [.text, .secret, .choice, .multi, .confirm, .code]
}

/// 选择题的一项。`value` 原样回传，App 不解释它。
struct PromptOption: Decodable, Sendable, Equatable, Identifiable {
    var value: String
    var label: String
    var note: String

    var id: String { value }

    enum CodingKeys: String, CodingKey { case value, label, note }

    init(value: String, label: String, note: String = "") {
        self.value = value
        self.label = label
        self.note = note
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        value = c.stringish(.value) ?? ""
        label = c.string(.label) ?? ""
        note = c.string(.note) ?? ""
    }
}

/// 一页里的一个字段。问什么、默认值、哪里不对都由 cca 给，App 只负责显示和把答案交回去。
struct PromptField: Decodable, Sendable, Equatable, Identifiable {
    var id: String
    var kind: PromptKind
    var label: String
    var defaultValue: String
    /// 重问时用户上次填的（密文永远是空的）。
    var value: String
    /// 上次的答案为什么不行。
    var error: String
    var hint: String
    var optional: Bool
    var options: [PromptOption]
    var min: Int?
    var detail: String
    var severity: Severity?
    /// 选择题“不选”那一项的说法。
    var skip: String
    /// 分组的小标题：相邻的同一组放在一起。
    var section: String
    /// 选择题的样子：`menu` 是一行里的弹出菜单（字段多的表单），别的是单选列表。
    var style: String

    enum CodingKeys: String, CodingKey {
        case id, kind, label
        case defaultValue = "default"
        case value, error, hint, optional, options, min, detail, severity, skip, section, style
    }

    init(id: String, kind: PromptKind, label: String = "", defaultValue: String = "", value: String = "", error: String = "",
         hint: String = "", optional: Bool = false, options: [PromptOption] = [], min: Int? = nil, detail: String = "",
         severity: Severity? = nil, skip: String = "", section: String = "", style: String = "") {
        self.id = id
        self.kind = kind
        self.label = label
        self.defaultValue = defaultValue
        self.value = value
        self.error = error
        self.hint = hint
        self.optional = optional
        self.options = options
        self.min = min
        self.detail = detail
        self.severity = severity
        self.skip = skip
        self.section = section
        self.style = style
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = c.stringish(.id) ?? ""
        kind = c.lossy(PromptKind.self, .kind) ?? PromptKind(rawValue: "")
        label = c.string(.label) ?? ""
        defaultValue = c.stringish(.defaultValue) ?? ""
        value = c.stringish(.value) ?? ""
        error = c.string(.error) ?? ""
        hint = c.string(.hint) ?? ""
        optional = c.bool(.optional) ?? false
        options = c.lossyArray(PromptOption.self, .options) ?? []
        min = c.int(.min)
        detail = c.string(.detail) ?? ""
        severity = c.lossy(Severity.self, .severity)
        skip = c.string(.skip) ?? ""
        section = c.string(.section) ?? ""
        style = c.string(.style) ?? ""
    }
}

/// `ask` 事件：cca 在等这一页的答案（每个字段一行，写进答案管道）。
struct AskEvent: Sendable, Equatable {
    var seq: Int
    var q: String
    var title: String
    var message: String
    var fields: [PromptField]
    /// 提交按钮的说法（空的是“继续”）。
    var submit: String = ""

    /// 这一页只有一个“是 / 否”的问题。
    var confirmField: PromptField? {
        fields.count == 1 && fields[0].kind == .confirm ? fields[0] : nil
    }

    /// App 能不能显示这一页：字段类型都认识、选择题有选项。不能显示的不能丢，要退回终端。
    var isDisplayable: Bool {
        !fields.isEmpty && fields.allSatisfy { f in
            guard f.kind.isKnown, !f.id.isEmpty else { return false }
            return (f.kind != .choice && f.kind != .multi) || !f.options.isEmpty
        }
    }
}

enum PromptAPI {
    /// App 支持的提问会话版本（`hello.prompt`，PROTOCOL 4.6）。
    static let version = 1
    /// 配置路由器、配置本机 Clash、账号表能在 App 里填表（`hello.forms`）：旧版 cca 问的是终端里的那一套，这几条走终端。
    static let forms = 1
}
