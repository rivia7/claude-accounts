import SwiftUI

/// 一个账号的详情：身份和主要操作（打开 App、网页版）、“现在能不能用、下一步点什么”的一句话、用量、网络出口。
/// 操作放在它作用的那一节旁边：刷新用量在“用量”的标题上，检测出口在“网络出口”的标题上。第一次来时顶上有一张提示卡片。
/// 窗口宽的时候（全屏）用量和网络出口左右并排，窄了上下排（SectionColumns）。
struct AccountDetail: View {
    @Environment(AppModel.self) private var model
    let ref: AccountRef

    var body: some View {
        if let a = model.account(ref) {
            ScrollView {
                VStack(alignment: .leading, spacing: 32) {
                    TipCard(id: "account-page", title: "这一页怎么看",
                            message: "下面这张卡片就是这个账号：名字旁边是套餐，卡片最下面那句话告诉你它现在能不能用、下一步点什么。再往下是额度和它的网络出口。想看端口、纯净度这些技术细节，点「显示细节」。")
                    DetailHeader(account: a)
                    SectionColumns {
                        if !a.isAPI { UsageSection(account: a) }
                        RouteSection(account: a)
                    }
                }
                .padding(.top, 24)
                .padding(.bottom, 36)
                .pageColumn()
            }
            .navigationTitle(a.displayName)
        } else {
            ContentUnavailableView("没有这个账号", systemImage: "person.crop.circle.badge.questionmark",
                                   description: Text("账号表里已经没有它了。"))
        }
    }
}

/// 头部是一张工牌（EmployeeBadgeCard）：头像、名字、邮箱和几个标签；右边是打开 App（主按钮）和网页版，没登录时多一个登录。
/// 宽的时候左右排，窄的时候按钮放到下面。正在打开、检测时下面多一行进度（来自 step 事件）。
struct DetailHeader: View {
    @Environment(AppModel.self) private var model
    @Environment(Guide.self) private var guide
    let account: Account

    var body: some View {
        let a = account
        EmployeeBadgeCard(account: a) {
            content(a)
        }
    }

    private func content(_ a: Account) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            ViewThatFits(in: .horizontal) {
                HStack(alignment: .center, spacing: 16) {
                    identity(a)
                    Spacer(minLength: 12)
                    actions(a)
                }
                VStack(alignment: .leading, spacing: 16) {
                    identity(a)
                    actions(a)
                }
            }
            if let busy = model.activity(a.ref) {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(Texts.activity(busy, appLabel: model.appLabel(a.tool))).font(.callout).foregroundStyle(.secondary)
                }
                .accessibilityElement(children: .combine)
            } else {
                // 给新手的一句话：现在能不能用、下一步点哪个按钮（和时间有关，每 30 秒重算）
                TimelineView(.periodic(from: .now, by: 30)) { context in
                    AdviceLine(advice: Advice.make(a, usage: model.usage(for: a), live: model.liveSlot(for: a.ref),
                                                   appLabel: model.appLabel(a.tool), now: context.date.timeIntervalSince1970))
                }
            }
            let reasons = disabledReasons(a)
            if !reasons.isEmpty {
                Label(reasons.joined(separator: "；"), systemImage: "info.circle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func identity(_ a: Account) -> some View {
        HStack(alignment: .center, spacing: 16) {
            AccountAvatar(account: a, size: 56, dot: 15, visible: model.mainVisible)
            VStack(alignment: .leading, spacing: 5) {
                AliasTitle(account: a)
                if let sub = secondaryIdentity(a) {
                    Text(sub)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .textSelection(.enabled)
                }
                HStack(spacing: 6) { tags(a) }
                    .padding(.top, 2)
            }
        }
    }

    /// 名字下面那行开头：有别名时是邮箱（名字已经是别名了）。
    private func secondaryIdentity(_ a: Account) -> String? {
        guard !a.isAPI, let email = nonEmpty(a.email), email != a.displayName else { return nil }
        return email
    }

    /// 标签：套餐徽章（最高档是整块渐变）；登录有问题时才说登录状态；在用（App 开着、命令行里几个会话）；上次在命令行里用的。
    @ViewBuilder
    private func tags(_ a: Account) -> some View {
        if a.isAPI {
            Tag(text: "第三方 API", color: .blue, systemImage: "key.horizontal")
            if let m = nonEmpty(a.api?.model) { Tag(text: m) }
        } else {
            if let p = nonEmpty(a.plan) { PlanBadge(plan: p, tier: a.planTier) }
            if let c = a.loginIssueColor {
                Tag(text: Texts.login(a.login, loginEmail: a.loginEmail), color: c)
            }
        }
        if a.isWorking { Tag(text: Texts.working(a, appLabel: model.appLabel(a.tool)), color: .green) }
        if a.isLast, guide.showDetails { Tag(text: "上次在命令行里用的", systemImage: "terminal") }
    }

    /// 打开 App（主按钮）、网页版；没登录或登错号时，“登录…”是主按钮。第三方 API 账号没有这两个按钮。
    @ViewBuilder
    private func actions(_ a: Account) -> some View {
        let busy = model.activity(a.ref) != nil
        let label = model.appLabel(a.tool)
        let needsLogin = a.login == .loggedOut || a.login == .emailMismatch
        if needsLogin || !a.isAPI {
            GlassRow(spacing: 8) {
                if needsLogin {
                    Button {
                        model.runInTerminal(model.loginHandoff(a))
                    } label: {
                        Label(a.login == .emailMismatch ? "重新登录…" : "登录…", systemImage: "person.badge.key")
                    }
                    .buttonStyle(.glassProminent)
                    .help("在终端里运行 \(model.loginHandoff(a).display)")
                }
                if !a.isAPI {
                    Button {
                        model.openApp(a.ref)
                    } label: {
                        Label("打开 \(label)", systemImage: "macwindow")
                    }
                    .glassButton(prominent: !needsLogin)
                    .disabled(!a.can.app || busy)
                    .help(reason("app", a) ?? "用这个账号的出口和独立的资料目录打开 \(label)")
                    Button {
                        model.openWeb(a.ref)
                    } label: {
                        Label("网页版", systemImage: "safari")
                    }
                    .buttonStyle(.glass)
                    .disabled(!a.can.web || busy)
                    .help(reason("web", a) ?? "在这个账号自己的 Chrome 资料里打开网页版，走账号的出口")
                }
            }
            .controlSize(.large)
        }
    }

    /// 按钮为什么不能用（cca 给的原因码）。API 账号没有这两个按钮，不说。
    private func disabledReasons(_ a: Account) -> [String] {
        if a.isAPI { return [] }
        var out: [String] = []
        let label = model.appLabel(a.tool)
        if !a.can.app, let w = a.canWhy["app"] { out.append("打开 App：" + Texts.canWhy(w, appLabel: label, tool: a.tool)) }
        if !a.can.web, let w = a.canWhy["web"] { out.append("网页版：" + Texts.canWhy(w, appLabel: label, tool: a.tool)) }
        return out
    }

    private func reason(_ action: String, _ a: Account) -> String? {
        guard let w = a.canWhy[action] else { return nil }
        let allowed: Bool
        switch action {
        case "app": allowed = a.can.app
        case "web": allowed = a.can.web
        default: allowed = a.can.usage
        }
        return allowed ? nil : Texts.canWhy(w, appLabel: model.appLabel(a.tool), tool: a.tool)
    }

}

/// 账号页的工牌：顶上一条账号色的带子（写着哪个组；显示细节时还有工号，就是它的槽位组 CCA-n），下面是头像、名字、套餐、
/// 按钮和那句话。最高档的套餐镶一圈 Logo 的渐变，第三方 API 账号是虚线边。内容层，是实的（不是玻璃）。
struct EmployeeBadgeCard<Content: View>: View {
    @Environment(Guide.self) private var guide
    let account: Account
    @ViewBuilder let content: Content

    var body: some View {
        let a = account
        let shape = RoundedRectangle(cornerRadius: 20, style: .continuous)
        let c = a.lineColor
        VStack(alignment: .leading, spacing: 0) {
            ZStack {
                HStack(spacing: 8) {
                    Text(Texts.department(Employee.department(of: a))).font(.caption.weight(.semibold))
                    Spacer(minLength: 8)
                    if guide.showDetails, let g = a.group {
                        Text(g).font(.caption.weight(.medium)).monospacedDigit()
                    }
                }
                // 挂绳的卡槽
                Capsule().fill(.black.opacity(0.28)).frame(width: 42, height: 7)
            }
            .foregroundStyle(.white)
            .padding(.horizontal, 16)
            .frame(height: 30)
            .background(LinearGradient(colors: [c, c.mix(with: .black, by: 0.18)], startPoint: .leading, endPoint: .trailing))
            .overlay(alignment: .bottom) {
                if a.isPremium { Rectangle().fill(TierStyle.line).frame(height: 2) }
            }
            .accessibilityElement(children: .combine)
            content
                .padding(18)
        }
        .background(shape.fill(Surface.tile))
        .clipShape(shape)
        .overlay {
            if a.isPremium {
                shape.strokeBorder(TierStyle.edge, lineWidth: 1.5)
            } else if a.isAPI {
                shape.strokeBorder(Surface.hairline, style: StrokeStyle(lineWidth: 1, dash: [5, 4]))
            } else {
                shape.strokeBorder(Surface.hairline, lineWidth: 0.5)
            }
        }
    }
}

extension View {
    /// 主按钮用 .glassProminent，其它 .glass。
    @ViewBuilder
    func glassButton(prominent: Bool) -> some View {
        if prominent {
            self.buttonStyle(.glassProminent)
        } else {
            self.buttonStyle(.glass)
        }
    }
}

/// 账号页的名字：直接点一下就能改别名（就地变成输入框，回车保存、Esc 取消）。悬停时露出淡淡的底色和笔。
struct AliasTitle: View {
    @Environment(AppModel.self) private var model
    let account: Account
    private let hover = State(initialValue: false)

    var body: some View {
        let a = account
        if model.aliasEditing == a.ref {
            InlineAliasEditor(account: a)
        } else {
            let on = hover.wrappedValue && model.activity(a.ref) == nil
            Button {
                model.beginRename(a.ref)
            } label: {
                HStack(spacing: 8) {
                    Text(a.displayName)
                        .font(.system(size: 24, weight: .semibold))
                        .foregroundStyle(.primary)
                        .lineLimit(1)
                    Image(systemName: "pencil")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .opacity(on ? 1 : 0.4)
                }
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color.primary.opacity(on ? 0.06 : 0)))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .padding(.horizontal, -6)
            .onHover { hover.wrappedValue = $0 }
            .disabled(model.activity(a.ref) != nil)
            .help("点一下改别名（登录、资料和端口都不变）")
            .accessibilityLabel("\(a.displayName)，点一下改别名")
        }
    }
}

/// 改别名的输入框：回车或 ✓ 保存（交给 cca rename，规则由 cca 判断），Esc 或 ✕ 取消；cca 说不行时原因写在下面，接着改。
struct InlineAliasEditor: View {
    @Environment(AppModel.self) private var model
    let account: Account
    /// 输入框的内容（命令行工具编不了 @State 宏：State 类型直接用，照样是视图的状态）
    private let draft: State<String>
    @FocusState private var focused: Bool

    init(account: Account) {
        self.account = account
        draft = State(initialValue: InlineAliasEditor.currentAlias(account))
    }

    static func currentAlias(_ a: Account) -> String {
        nonEmpty(a.alias) ?? (a.isAPI ? a.key : "")
    }

    var body: some View {
        let a = account
        let busy = model.activity(a.ref) != nil
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 8) {
                TextField(a.isAPI ? "别名" : "别名（空着就只用邮箱）", text: draft.projectedValue)
                    .textFieldStyle(.plain)
                    .font(.system(size: 22, weight: .semibold))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .frame(width: 280)
                    .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(.background))
                    .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .strokeBorder(model.renameError == nil ? Color.accentColor : Color.red, lineWidth: 1.5))
                    .focused($focused)
                    .disabled(busy)
                    .onSubmit { save() }
                    .onExitCommand { model.cancelRename() }
                if busy {
                    ProgressView().controlSize(.small)
                } else {
                    Button { save() } label: { Image(systemName: "checkmark") }
                        .buttonStyle(.borderless)
                        .help("保存（回车）")
                        .accessibilityLabel("保存别名")
                    Button { model.cancelRename() } label: { Image(systemName: "xmark") }
                        .buttonStyle(.borderless)
                        .help("取消（Esc）")
                        .accessibilityLabel("取消改名")
                }
            }
            if let e = model.renameError {
                Label(e, systemImage: "exclamationmark.circle.fill")
                    .font(.caption)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            } else {
                Text(a.isAPI ? "回车保存，Esc 取消。最多 12 个汉字或 24 个字母，不能有空格、引号、括号、反斜杠或 @。"
                     : "回车保存，Esc 取消。最多 12 个汉字或 24 个字母，不能有空格、引号、括号、反斜杠或 @；空着就只用邮箱。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .onAppear { DispatchQueue.main.async { focused = true } }
    }

    private func save() {
        let text = draft.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard model.activity(account.ref) == nil else { return }
        if text == InlineAliasEditor.currentAlias(account) {
            model.cancelRename()
            return
        }
        model.rename(account.ref, to: text)
    }
}
