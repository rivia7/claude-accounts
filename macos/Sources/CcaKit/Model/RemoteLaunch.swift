import Foundation

// 远程启动（PROTOCOL 9.4）：新建、修改模板的弹层（RemoteTemplateDraft）。模板的参数在 App 里是一行字：cca 拼好的 `args_line`，
// 存的时候原样交给 cca 拆（`--args-line`），App 不自己拆、拼。

/// 新建、修改一个模板的弹层里填的东西（这套工具链里 @State 用不了：放在被观察的对象里）。存的时候交给 cca 拆参数、校验，
/// 不行的原因（cca 说的）写在弹层里，接着改。
@MainActor @Observable
final class RemoteTemplateDraft: Identifiable {
    nonisolated let id = UUID()
    /// 改的是哪一个（新建时是 nil）：存的时候带 `--replace <编号>`
    let original: RemoteTemplate?
    var name: String
    var tool: ToolID
    /// 账号表的键（账号页里的那个）；还没选是空的
    var account: String
    /// 文件夹（绝对路径）；还没选是空的
    var dir: String
    /// 参数：一行字，照 shell 的写法（cca 拆）
    var args: String
    /// 没存成的原因（写在弹层里）
    var error: String?
    /// 正在存（按钮先停用）
    var saving = false

    init(original: RemoteTemplate?, name: String, tool: ToolID, account: String, dir: String, args: String) {
        self.original = original
        self.name = name
        self.tool = tool
        self.account = account
        self.dir = dir
        self.args = args
    }

    /// 名字、账号、文件夹都有了才能存（别的交给 cca 校验）
    var canSave: Bool {
        !saving && !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !account.isEmpty && !dir.isEmpty
    }
}

extension RemoteTemplateDraft: Equatable {
    nonisolated static func == (a: RemoteTemplateDraft, b: RemoteTemplateDraft) -> Bool { a === b }
}
