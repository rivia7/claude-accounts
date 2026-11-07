import SwiftUI

/// 改别名：输入框里是现在的别名，空着就只用邮箱（第三方 API 账号必须有别名）。规则由 cca 判断（`cca rename`），
/// 它说不行时原因写在输入框下面，弹层不关；改好了弹层自己关上、选中改名后的账号。
struct RenameSheet: View {
    @Environment(AppModel.self) private var model
    let account: Account
    /// 输入框的内容（命令行工具编不了 @State 宏：State 类型直接用，照样是视图的状态）
    private let draft: State<String>

    init(account: Account) {
        self.account = account
        draft = State(initialValue: RenameSheet.currentAlias(account))
    }

    static func currentAlias(_ a: Account) -> String {
        nonEmpty(a.alias) ?? (a.isAPI ? a.key : "")
    }

    var body: some View {
        let a = account
        let busy = model.activity(a.ref) != nil
        let text = draft.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines)
        let unchanged = text == RenameSheet.currentAlias(a)
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 12) {
                BadgeAvatar(badge: a.badge, isAPI: a.isAPI, size: 40)
                VStack(alignment: .leading, spacing: 2) {
                    Text("改别名").font(.title3.weight(.semibold))
                    Text(subtitle(a)).font(.callout).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            VStack(alignment: .leading, spacing: 8) {
                TextField(a.isAPI ? "别名" : "别名（空着就只用邮箱）", text: draft.projectedValue)
                    .textFieldStyle(.roundedBorder)
                    .font(.body)
                    .onSubmit { submit(text, unchanged: unchanged) }
                    .disabled(busy)
                Text("列表、菜单栏、启动器和命令行里的 @别名 都用它；改名不影响登录、浏览器资料和端口。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Text(a.isAPI ? "最多 12 个汉字或 24 个字母，不能有空格、引号、括号、反斜杠或 @。"
                     : "最多 12 个汉字或 24 个字母，不能有空格、引号、括号、反斜杠或 @。空着就只用邮箱。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let e = model.renameError {
                    Label(e, systemImage: "exclamationmark.circle.fill")
                        .font(.callout)
                        .foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
            }
            HStack(spacing: 10) {
                if busy {
                    ProgressView().controlSize(.small)
                    Text("正在改名…").font(.callout).foregroundStyle(.secondary)
                }
                Spacer()
                Button("取消") { model.dismissSheet() }
                    .keyboardShortcut(.cancelAction)
                Button("改名") { submit(text, unchanged: unchanged) }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.borderedProminent)
                    .disabled(busy || unchanged)
            }
        }
        .padding(22)
        .frame(width: 440)
    }

    private func subtitle(_ a: Account) -> String {
        if a.isAPI { return "第三方 API · " + (a.api?.host ?? a.key) }
        return nonEmpty(a.email) ?? a.key
    }

    private func submit(_ text: String, unchanged: Bool) {
        guard !unchanged, model.activity(account.ref) == nil else { return }
        model.rename(account.ref, to: text)
    }
}
