import SwiftUI

/// API key 那一行（官方账号在「用量」里，「订阅附带的每月 API 额度」下面；第三方 API 账号在自己的「API key」一节里）：
/// 掩码（cca 存的时候算好的，`sk-ant-…x7Qa`）和一只眼睛——点开时才向 cca 要一次明文，显示在下面，30 秒后、再点一下、
/// 换了账号、离开这一页时收起来；右边是「复制」（剪贴板标成机密，60 秒后清掉）。还没存时右边是「记录…」（官方账号）
/// 或「输入…」（第三方 API 账号）。官方账号记下的那一把会加密同步给登录了这个账号的其他设备时，下面一句说来源或同步的情况
/// （`Texts.apiKeyShare`）。明文只在 KeyReveal 里（不被观察、不进读屏），这一行自己的状态，不放进 AppModel。
/// 用的地方要加 `.id(a.ref)`：换了账号就是一个新的 KeyReveal（读到一半的结果不会显示在别的账号下面）。
struct APIKeyRow: View {
    @Environment(AppModel.self) private var model
    let account: Account
    /// 这一行左边的说明（第三方 API 账号的「API key」一节里写 key 存在哪，免得和一节的标题重复）
    var title = "API key"
    /// 命令行工具编不了 @State 宏：State 类型直接用
    private let reveal = State(initialValue: KeyReveal())

    var body: some View {
        let a = account
        let info = a.apiKey ?? APIKeyInfo(saved: false, hint: "", source: .none)
        let r = reveal.wrappedValue
        let busy = model.activity(a.ref) != nil
        VStack(alignment: .leading, spacing: 6) {
            InfoRow(Text(title), systemImage: "key", value: { value(a, info, r) }) {
                if info.saved {
                    RowAction("复制", systemImage: "doc.on.doc") { model.copyAPIKey(a) }
                        .help("复制到剪贴板：标成机密（剪贴板管理器不记），60 秒后自动清掉")
                } else {
                    RowAction(a.isAPI ? "输入…" : "记录…", systemImage: "plus") { model.recordAPIKey(a) }
                        .disabled(busy)
                        .help(Texts.apiKeyRecordHelp(isAPI: a.isAPI, shared: info.shared))
                }
            }
            if r.phase == .shown, let s = r.secret {
                Text(verbatim: s.text)
                    .font(.callout.monospaced())
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 7)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Surface.tile))
                    .accessibilityLabel("API key 已显示，30 秒后自动隐藏")
            }
            // 会加密同步给登录了这个账号的其他设备时（cca 说的）：从哪台设备来的、别的设备也能看到、没同步成的原因。
            // 图标和文字跟上面那一行（InfoRow）的对齐
            if !a.isAPI, let note = Texts.apiKeyShare(info, now: Date().timeIntervalSince1970) {
                Label(note, systemImage: info.syncError == nil ? "arrow.triangle.2.circlepath" : "exclamationmark.triangle")
                    .labelStyle(FixedIconLabelStyle(iconWidth: 20, spacing: 8))
                    .font(.caption)
                    .foregroundStyle(info.syncError == nil ? AnyShapeStyle(.secondary) : AnyShapeStyle(.orange))
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .onDisappear { r.hide() }
        .onChange(of: info) { r.hide() }
    }

    /// 掩码和眼睛；没存时写一句
    @ViewBuilder
    private func value(_ a: Account, _ info: APIKeyInfo, _ r: KeyReveal) -> some View {
        if info.saved {
            HStack(spacing: 6) {
                Text(verbatim: nonEmpty(info.hint) ?? "••••••••")
                    .font(.callout.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Button {
                    r.toggle { [model] in await model.readAPIKey(a) }
                } label: {
                    if r.phase == .loading {
                        ProgressView().controlSize(.mini).frame(width: 16)
                    } else {
                        Image(systemName: r.phase == .shown ? "eye.slash" : "eye").frame(width: 16)
                    }
                }
                .buttonStyle(.borderless)
                .help(r.phase == .shown ? "隐藏" : "显示完整的 key（30 秒后自动隐藏）")
                .accessibilityLabel(r.phase == .shown ? "隐藏 API key" : "显示 API key")
            }
        } else {
            Text(a.isAPI ? "还没有" : "还没记录").foregroundStyle(.secondary)
        }
    }
}

/// 第三方 API 账号的「API key」一节（它们没有「用量」）：一行 APIKeyRow，标题右边是「更换…」；key 写在账号表里（明文）时说一句。
struct APIKeySection: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        let a = account
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader("API key") {
                if a.apiKey?.saved == true {
                    SectionAction("更换…", systemImage: "key") { model.recordAPIKey(a) }
                        .disabled(model.activity(a.ref) != nil)
                        .help("输入新的 key，存进这台 Mac 的钥匙串")
                }
            }
            APIKeyRow(account: a, title: a.apiKey?.source == .table ? "账号表里写的 key" : "钥匙串里的 key").id(a.ref)
            if a.apiKey?.source == .table {
                Text("这把 key 写在账号表里（明文），启动时优先用它；要换请用 \(Handoff.edit(prefix: model.prefix(a.tool)).display) 改账号表。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}
