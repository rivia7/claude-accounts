import SwiftUI

/// 账号页的「邮箱」一节（邮箱助手，PROTOCOL 4.11）：登录时 Anthropic 发来的登录邮件一到转发的 Gmail，cca 就在这个账号专属的浏览器里
/// 打开里面的链接。这一节让你配置收登录邮件的 Gmail（默认所有账号共用，也能给这个账号单独配一个），并按需看最近的登录邮件、
/// 在浏览器里打开某一封。登录链接是一次性凭证，始终不交给 App：列表只有元数据和链接的“样子”，打开由 cca 自己完成。
/// 只有 cca 够新（`hello.mail`）、这个账号 `can.mail` 时才显示。
struct MailSection: View {
    @Environment(AppModel.self) private var model
    let account: Account
    /// 展开后才按需拉登录邮件（不随开页自动拉，免得翻账号变慢）
    private let expanded = State(initialValue: false)
    /// 移除默认邮箱的确认（影响所有账号）
    private let confirmRemoveDefault = State(initialValue: false)

    var body: some View {
        let mail = account.mail
        let snap = model.mailLists[account.ref]
        let loading = model.mailLoading.contains(account.ref)
        let configured = mail?.configured == true
        // 拉过一次（有缓存）或手动展开后才显示列表；没缓存的新账号页先折叠（不自动拉，翻账号不卡）
        let showList = configured && (expanded.wrappedValue || snap?.doc != nil || loading)
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader("邮箱") {
                if showList {
                    TimelineView(.periodic(from: .now, by: 30)) { context in
                        freshness(snap, loading: loading, now: context.date.timeIntervalSince1970)
                    }
                    SectionAction(loading ? "查询中…" : "刷新", systemImage: "arrow.clockwise") { model.refreshMail(account.ref) }
                        .disabled(loading)
                        .help("再查一次这个 Gmail 里最近的登录邮件")
                }
                configMenu(mail)
            }
            statusRow(mail)
            if showList {
                TimelineView(.periodic(from: .now, by: 30)) { context in
                    listContent(snap, loading: loading, now: context.date.timeIntervalSince1970)
                }
            } else if configured {
                Button {
                    expanded.wrappedValue = true
                    if model.mailLists[account.ref]?.doc == nil { model.refreshMail(account.ref) }
                } label: {
                    Label("查看最近的登录邮件", systemImage: "tray.and.arrow.down")
                        .labelStyle(FixedIconLabelStyle(iconWidth: RowMetrics.actionIconWidth, spacing: 6))
                }
                .buttonStyle(.glass)
                .controlSize(.regular)
                .help("连一次这个 Gmail，列出最近 24 小时的登录相关邮件（只读，不标已读）")
                .padding(.horizontal, 4)
            }
        }
        .confirmationDialog("移除默认邮箱？", isPresented: Binding(get: { confirmRemoveDefault.wrappedValue },
                                                           set: { confirmRemoveDefault.wrappedValue = $0 }), titleVisibility: .visible) {
            Button("移除默认邮箱", role: .destructive) { model.mailForget(account.ref, scope: .default) }
            Button("取消", role: .cancel) {}
        } message: {
            Text("所有用默认 Gmail 收登录邮件的账号都会受影响（单独配置过的账号不受影响）。")
        }
    }

    // MARK: 配置菜单

    @ViewBuilder
    private func configMenu(_ mail: MailConfig?) -> some View {
        let busy = model.activity(account.ref) != nil
        Menu {
            Button("配置默认邮箱（所有 Claude 账号共用）…") { model.mailSetup(nil) }
            Button(mail?.isAccountSpecific == true ? "更换这个账号单独用的邮箱…" : "给这个账号单独配置邮箱…") { model.mailSetup(account.ref) }
            if mail?.isAccountSpecific == true {
                Divider()
                Button("改回默认邮箱", role: .destructive) { model.mailForget(account.ref, scope: .account) }
            } else if mail?.configured == true {
                Divider()
                Button("移除默认邮箱（所有账号）…", role: .destructive) { confirmRemoveDefault.wrappedValue = true }
            }
        } label: {
            Label(mail?.configured == true ? "配置" : "配置邮箱…", systemImage: "gearshape")
        }
        .menuStyle(.button)
        .buttonStyle(.glass)
        .controlSize(.regular)
        .fixedSize()
        .disabled(busy)
        .help("配置收登录邮件的 Gmail（应用专用密码只存进这台 Mac 的钥匙串）")
    }

    // MARK: 状态行

    @ViewBuilder
    private func statusRow(_ mail: MailConfig?) -> some View {
        let icon = mail?.configured == true ? "envelope" : "envelope.badge"
        InfoRow(Text(statusText(mail)), systemImage: icon) {
            if mail?.configured == true, mail?.isAccountSpecific == true {
                Tag(text: "单独配置")
            } else if mail?.configured == true {
                Tag(text: "默认")
            }
        } actions: {
            EmptyView()
        }
    }

    private func statusText(_ mail: MailConfig?) -> String {
        guard let mail, mail.configured, let user = nonEmpty(mail.user) else {
            return "还没配置：把这个账号的邮箱转发到一个 Gmail，登录时 cca 就在它专属的浏览器里打开登录邮件里的链接"
        }
        let where_ = mail.isAccountSpecific ? "这个账号单独用 \(user) 收登录邮件" : "登录邮件走默认 Gmail \(user)"
        let auto = mail.auto ? "，登录时自动打开里面的链接" : "，登录时不自动打开（用下面的列表手动打开）"
        return where_ + auto
    }

    // MARK: 登录邮件列表

    @ViewBuilder
    private func freshness(_ s: MailListSnapshot?, loading: Bool, now: Double) -> some View {
        if loading, s?.doc == nil {
            ProgressView().controlSize(.small)
        } else if let at = s?.fetchedEpoch {
            Text(Texts.agoPhrase(now - at) + "查询")
                .font(.caption)
                .foregroundStyle(.secondary)
                .monospacedDigit()
        }
    }

    @ViewBuilder
    private func listContent(_ s: MailListSnapshot?, loading: Bool, now: Double) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            // 硬错误（连不上 cca）
            if let e = s?.error {
                Banner(systemImage: "exclamationmark.triangle", tint: .orange, title: "没能查登录邮件", message: e) { EmptyView() }
            }
            // 软错误（没配置、没密码、连不上 Gmail）
            if let err = s?.doc?.error {
                Banner(systemImage: "envelope.badge", tint: .orange, title: "查不了这个 Gmail", message: err.text) {
                    if err.code == "no_password" || err.code == "not_configured" {
                        Button("配置…") { model.mailSetup(account.ref) }.buttonStyle(.borderedProminent).controlSize(.small)
                    }
                }
            } else if let doc = s?.doc {
                if doc.rows.isEmpty {
                    if loading {
                        ProgressView().controlSize(.small).padding(.vertical, 4)
                    } else {
                        Label("最近 24 小时里没有登录相关的邮件。在登录页上用邮箱登录，收到后点「刷新」", systemImage: "tray")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                } else {
                    ForEach(doc.rows) { row in
                        MailRowView(account: account, row: row, now: now)
                    }
                    if doc.truncated {
                        Text("只显示最近的 50 封").font(.caption).foregroundStyle(.secondary)
                    }
                }
            } else if loading {
                ProgressView().controlSize(.small).padding(.vertical, 4)
            }
            Text("经 IMAP 只读：不改、不删、不标已读。登录链接是一次性凭证，只交给这个账号的浏览器，这里只显示它的“样子”")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 4)
    }
}

/// 一封登录相关的邮件：时间、标题、核对结论；通过核对的给一个「在浏览器打开」。
struct MailRowView: View {
    @Environment(AppModel.self) private var model
    let account: Account
    let row: MailRow
    let now: Double

    var body: some View {
        let busy = model.activity(account.ref) != nil
        let hasChrome = model.toolState(account.tool)?.chrome ?? true
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 10) {
                verdictDot
                Text(row.subjectText)
                    .fontWeight(.medium)
                    .lineLimit(1)
                    .truncationMode(.tail)
                if let folder = nonEmpty(row.folder) { Tag(text: folder) }
                Spacer(minLength: 8)
                if let got = row.gotEpoch {
                    Text(Date(timeIntervalSince1970: got), format: .dateTime.month().day().hour().minute())
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                }
                if row.openable, let h = row.msg {
                    RowAction.web(hasChrome ? "在浏览器打开" : "需要 Chrome") { model.openMailMessage(account.ref, h) }
                        .disabled(busy || !hasChrome)
                        .help(hasChrome ? "cca 重新取这封、再核对一遍，在这个账号专属的 Chrome 里打开里面的登录链接"
                              : "没装 Google Chrome：登录链接要在账号专属的 Chrome 里打开")
                }
            }
            Text(detail)
                .font(.caption)
                .foregroundStyle(row.verdict.ok ? AnyShapeStyle(.secondary) : AnyShapeStyle(Color.orange))
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private var verdictDot: some View {
        Circle()
            .fill(row.verdict.ok ? Color.green : Color.secondary.opacity(0.4))
            .frame(width: 8, height: 8)
            .accessibilityHidden(true)
    }

    /// 核对结论那一行：通过时说发件人和链接的样子；没通过说原因。
    private var detail: String {
        if row.verdict.ok {
            var parts: [String] = []
            if let from = row.from.first, !from.isEmpty { parts.append("发件人 " + from) }
            if let shape = nonEmpty(row.linkShape) { parts.append(shape) }
            if parts.isEmpty { parts.append("通过核对") }
            return "✓ " + parts.joined(separator: " · ")
        }
        return "✗ " + (nonEmpty(row.verdict.whyText) ?? "没通过核对")
    }
}
