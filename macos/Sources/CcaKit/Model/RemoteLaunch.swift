import Foundation

// 远程启动（PROTOCOL 9.4）：新建、修改模板的弹层（RemoteTemplateDraft）。模板的参数在 App 里是一行字：cca 拼好的 `args_line`，
// 存的时候原样交给 cca 拆（`--args-line`），App 不自己拆、拼。
// 用配对过的另一台机器上的模板在那台机器上新建会话（PROTOCOL 9.5）：挑模板的弹层（RemoteLaunchDraft）。

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

/// 在配对过的另一台机器上新建会话的弹层里选的（PROTOCOL 9.5）：哪个模板、开场白。起不成的原因（cca 说的）写在弹层里，可以再起。
@MainActor @Observable
final class RemoteLaunchDraft: Identifiable {
    nonisolated let id = UUID()
    /// 那台机器的启动器：打开弹层时列表里的那一份（列表刷新了照这一份起；模板改过了 cca 说，再打开就是新的）
    let launcher: RemotePeerLauncher
    /// 选的模板的编号；还没选是 nil
    var template: String?
    /// 开场白（模板能带时才用）：起会话时交给那边的工具的第一句话
    var line = ""
    /// 没起成的原因（写在弹层里）
    var error: String?
    /// 正在起（按钮先停用）：要等那台机器起好、新会话在服务器上登记好，可能要几十秒
    var starting = false

    init(launcher: RemotePeerLauncher, template: String?) {
        self.launcher = launcher
        self.template = template
    }

    var chosen: RemotePeerTemplate? { template.flatMap { launcher.template($0) } }
    /// 要交给 cca 的开场白：模板能带、写了（不是空白）才有
    var sentLine: String? {
        guard chosen?.line == true else { return nil }
        return nonEmpty(line.trimmingCharacters(in: .whitespacesAndNewlines))
    }
    var canStart: Bool { !starting && chosen != nil && launcher.online }
}

extension RemoteLaunchDraft: Equatable {
    nonisolated static func == (a: RemoteLaunchDraft, b: RemoteLaunchDraft) -> Bool { a === b }
}
