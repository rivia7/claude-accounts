import Foundation

// 远程启动（PROTOCOL 9.4）：新建、修改模板的弹层（RemoteTemplateDraft）。模板的参数在 App 里是一行字：cca 拼好的 `args_line`，
// 存的时候原样交给 cca 拆（`--args-line`），App 不自己拆、拼。
// 用配对过的另一台机器上的模板在那台机器上新建会话（PROTOCOL 9.5）：挑模板、账号的弹层（RemoteLaunchDraft）。账号不在模板里（3.7.10 起）：
// 起的时候挑那台机器上这个工具的一个账号。

/// 新建、修改一个模板的弹层里填的东西（这套工具链里 @State 用不了：放在被观察的对象里）。存的时候交给 cca 拆参数、校验，
/// 不行的原因（cca 说的）写在弹层里，接着改。
@MainActor @Observable
final class RemoteTemplateDraft: Identifiable {
    nonisolated let id = UUID()
    /// 改的是哪一个（新建时是 nil）：存的时候带 `--replace <编号>`
    let original: RemoteTemplate?
    var name: String
    var tool: ToolID
    /// 文件夹（绝对路径）；还没选是空的
    var dir: String
    /// 参数：一行字，照 shell 的写法（cca 拆）
    var args: String
    /// 没存成的原因（写在弹层里）
    var error: String?
    /// 正在存（按钮先停用）
    var saving = false

    init(original: RemoteTemplate?, name: String, tool: ToolID, dir: String, args: String) {
        self.original = original
        self.name = name
        self.tool = tool
        self.dir = dir
        self.args = args
    }

    /// 名字、文件夹都有了才能存（别的交给 cca 校验）
    var canSave: Bool {
        !saving && !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !dir.isEmpty
    }
}

extension RemoteTemplateDraft: Equatable {
    nonisolated static func == (a: RemoteTemplateDraft, b: RemoteTemplateDraft) -> Bool { a === b }
}

/// 在配对过的另一台机器上新建会话的弹层里选的（PROTOCOL 9.5）：哪个模板、哪个账号、开场白。起不成的原因（cca 说的）写在弹层里，可以再起。
@MainActor @Observable
final class RemoteLaunchDraft: Identifiable {
    nonisolated let id = UUID()
    /// 那台机器的启动器：打开弹层时列表里的那一份（列表刷新了照这一份起；模板改过了 cca 说，再打开就是新的）
    let launcher: RemotePeerLauncher
    /// 选的模板的编号；还没选是 nil。换模板用 `choose(template:)`：账号跟着换成那个工具的
    private(set) var template: String?
    /// 选的账号（`RemotePeerAccount.id`）；还没选、那台机器上的 cca 是旧版（不用挑）是 nil
    var account: String?
    /// 开场白（模板能带时才用）：起会话时交给那边的工具的第一句话
    var line = ""
    /// 没起成的原因（写在弹层里）
    var error: String?
    /// 正在起（按钮先停用）：要等那台机器起好、新会话在服务器上登记好，可能要几十秒
    var starting = false

    init(launcher: RemotePeerLauncher, template: String?) {
        self.launcher = launcher
        choose(template: template)
    }

    var chosen: RemotePeerTemplate? { template.flatMap { launcher.template($0) } }
    /// 选的模板能挑的账号：还没选模板是空的；那台机器上的 cca 是旧版时是 nil（模板用它自己的账号）
    var choices: [RemotePeerAccount]? {
        guard let t = chosen else { return launcher.accounts == nil ? nil : [] }
        return launcher.accounts(for: t)
    }

    /// 换模板：选的账号不是这个工具的就不要了；只有一个能挑时先选上
    func choose(template: String?) {
        self.template = template
        let list = choices ?? []
        if let a = account, !list.contains(where: { $0.id == a }) { account = nil }
        if account == nil, list.count == 1 { account = list[0].id }
    }

    /// 要交给 cca 的账号（`--account`）：要挑的时候才有
    var sentAccount: String? { choices == nil ? nil : account }
    /// 要交给 cca 的开场白：模板能带、写了（不是空白）才有
    var sentLine: String? {
        guard chosen?.line == true else { return nil }
        return nonEmpty(line.trimmingCharacters(in: .whitespacesAndNewlines))
    }
    /// 选好了：模板、账号（要挑的时候）；那台机器连着服务器
    var canStart: Bool {
        guard !starting, chosen != nil, launcher.online else { return false }
        guard let list = choices else { return true }
        return account.map { a in list.contains { $0.id == a } } ?? false
    }
}

extension RemoteLaunchDraft: Equatable {
    nonisolated static func == (a: RemoteLaunchDraft, b: RemoteLaunchDraft) -> Bool { a === b }
}
