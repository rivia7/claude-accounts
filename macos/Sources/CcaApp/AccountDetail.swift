import SwiftUI

/// 一个账号的详情：身份和主要操作（打开 App、网页版）、“现在能不能用、下一步点什么”的一句话、用量、每周用量和折合金额（第三方 API 账号是
/// 「API key」）、网络出口；映射里的账号还有「设备」（在哪些设备上用，DevicesSection）。
/// 操作放在它作用的那一节旁边：刷新用量在“用量”的标题上，检测出口在“网络出口”的标题上。第一次来时顶上有一张提示卡片。
/// 窗口宽的时候（全屏）用量（连同每周用量）和网络出口左右并排，窄了上下排（SectionColumns）；「设备」总是在下面占一整行。
struct AccountDetail: View {
    @Environment(AppModel.self) private var model
    let ref: AccountRef

    var body: some View {
        if let a = model.account(ref) {
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 32) {
                        TipCard(id: "account-page", title: "这一页怎么看",
                                message: "名字下面那句话说这个账号现在能不能用、下一步点什么。"
                                    + (a.isAPI ? "往下是它的 API key 和网络出口。"
                                       : "往下是用量和网络出口；维护者映射里的账号，最下面还有它在哪些设备上用。")
                                    + "端口、纯净度这些细节，点「显示细节」。")
                        DetailHeader(account: a)
                        SectionColumns {
                            if !a.isAPI, !model.features(a.tool).usage {
                                NoUsageSection(account: a)   // Antigravity：没有查用量的接口
                            } else if !a.isAPI {
                                // 按官方 API 的价格折成多少美元（cca 有折合金额、这个账号有时）
                                let cost = model.costEnabled && a.can.cost
                                VStack(alignment: .leading, spacing: 32) {
                                    UsageSection(account: a)
                                    // 图的右边留出和下面折合金额的纵轴一样宽的一栏：同一周的条上下对齐
                                    WeeklyUsageSection(account: a, gutter: cost ? CostChart.gutterLabels(
                                        a.cost, granularity: model.costGranularity, scope: model.costScope) : [])
                                    if cost {
                                        CostSection(account: a).id(CostSection.anchor)
                                    }
                                }
                            } else if a.can.apiKey {
                                APIKeySection(account: a)
                            }
                            RouteSection(account: a)
                        }
                        // 映射里的账号：在哪些设备上用（整行，不进并排的两栏）
                        if a.can.devices { DevicesSection(account: a) }
                    }
                    .padding(.top, 24)
                    .padding(.bottom, 36)
                    .pageColumn()
                }
                // 折合金额页点了这个账号：滚到「折合金额」。id 是“要滚的是不是这个账号”：这一页换账号时
                // costFocus 和 ref 不一定在同一次更新里变，只看 costFocus 会拿着上一个账号的 ref 跑完、之后不再跑
                .task(id: model.costFocus == ref) {
                    guard model.costFocus == ref else { return }
                    // 滚完、换了账号、离开这一页都算处理过了（别的账号要滚的不动）
                    defer { if model.costFocus == ref { model.costFocus = nil } }
                    // 等这一页排好再滚；刚换了账号、工具时上面几节过一会儿才排定，再对一次
                    for delay: UInt64 in [80_000_000, 320_000_000] {
                        try? await Task.sleep(nanoseconds: delay)
                        guard !Task.isCancelled else { return }
                        proxy.scrollTo(CostSection.anchor, anchor: .top)
                    }
                }
            }
            .navigationTitle(a.displayName)
        } else {
            ContentUnavailableView("没有这个账号", systemImage: "person.crop.circle.badge.questionmark",
                                   description: Text("账号表里已经没有它了。"))
        }
    }
}

/// 头部：头像、名字、邮箱和几个标签；右边是打开 App（主按钮）和网页版，没登录时多一个登录。直接放在页面上，不套卡片、不上底色。
/// 宽的时候左右排，窄的时候按钮放到下面。正在打开、检测时下面多一行进度（来自 step 事件）。
struct DetailHeader: View {
    @Environment(AppModel.self) private var model
    @Environment(Guide.self) private var guide
    let account: Account

    var body: some View {
        let a = account
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
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    Label(reasons.joined(separator: "；"), systemImage: "info.circle")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    // 没装 claude / codex / agy 命令：在终端里用官方安装脚本装（cca 够新时才有）
                    if let h = model.installCLIHandoff(for: a) {
                        Button("安装 \(Texts.cliCommand(a.tool)) 命令…") { model.runInTerminal(h) }
                            .buttonStyle(.bordered)
                            .controlSize(.small)
                            .help("在终端里运行 \(h.display)：用官方安装脚本安装，直连不通时会问要不要经内置代理装")
                    }
                }
            }
        }
    }

    private func identity(_ a: Account) -> some View {
        HStack(alignment: .center, spacing: 16) {
            AccountAvatar(account: a, size: 56, dot: 15)
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

    /// 标签：套餐徽章；登录有问题时才说登录状态；在用（App 开着、命令行里几个会话）；上次在命令行里用的。
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
            if model.loginExpired(a) { Tag(text: Texts.usageError(.auth), color: .orange) }
        }
        if a.isWorking { Tag(text: Texts.working(a, appLabel: model.appLabel(a.tool)), color: .green) }
        if a.isLast, guide.showDetails { Tag(text: "上次在终端里用的", systemImage: "terminal") }
    }

    /// 打开 App（主按钮）、网页版、终端；没登录、登错号或登录失效时，“登录…”是主按钮。第三方 API 账号没有 App 和网页版，终端是它的主按钮。
    @ViewBuilder
    private func actions(_ a: Account) -> some View {
        let busy = model.activity(a.ref) != nil
        let label = model.appLabel(a.tool)
        let needsLogin = a.login == .loggedOut || a.login == .emailMismatch || model.loginExpired(a)
        let hasApp = model.features(a.tool).app
        GlassRow(spacing: 8) {
            if needsLogin {
                Button {
                    model.relogin(a)
                } label: {
                    Label(a.login == .loggedOut ? "登录…" : "重新登录…", systemImage: "person.badge.key")
                }
                .buttonStyle(.glassProminent)
                .help("在终端里运行 \(model.loginHandoff(a).display)")
            }
            if !a.isAPI {
                if hasApp {
                    Button {
                        model.openApp(a.ref)
                    } label: {
                        Label("打开 \(label)", systemImage: "macwindow")
                    }
                    .glassButton(prominent: !needsLogin)
                    .disabled(!a.can.app || busy)
                    .help(reason("app", a) ?? "用这个账号的出口和独立的资料目录打开 \(label)")
                }
                Button {
                    model.openWeb(a.ref)
                } label: {
                    Label("网页版", systemImage: "safari")
                }
                .buttonStyle(.glass)
                .disabled(!a.can.web || busy)
                .help(reason("web", a) ?? "在这个账号自己的 Chrome 资料里打开网页版，走账号的出口")
            }
            SessionButton(account: a, prominent: (a.isAPI || !hasApp) && !needsLogin, unavailable: reason("cli", a))
            Menu {
                AccountManageItems(account: a, busy: busy)
            } label: {
                Image(systemName: "ellipsis")
            }
            .menuIndicator(.hidden)
            .buttonStyle(.glass)
            .help("管理这个账号")
            .accessibilityLabel("管理这个账号")
        }
        .controlSize(.large)
    }

    /// 按钮为什么不能用（cca 给的原因码）。第三方 API 账号本来就没有 App 和网页版的按钮，不说。
    private func disabledReasons(_ a: Account) -> [String] {
        var out: [String] = []
        let label = model.appLabel(a.tool)
        if !a.isAPI {
            if !a.can.app, model.features(a.tool).app, let w = a.canWhy["app"] {
                out.append("打开 App：" + Texts.canWhy(w, appLabel: label, tool: a.tool))
            }
            if !a.can.web, let w = a.canWhy["web"] { out.append("网页版：" + Texts.canWhy(w, appLabel: label, tool: a.tool)) }
        }
        if !a.can.cli, let w = a.canWhy["cli"] { out.append("终端：" + Texts.canWhy(w, appLabel: label, tool: a.tool)) }
        return out
    }

    private func reason(_ action: String, _ a: Account) -> String? {
        guard let w = a.canWhy[action] else { return nil }
        let allowed: Bool
        switch action {
        case "app": allowed = a.can.app
        case "web": allowed = a.can.web
        case "cli": allowed = a.can.cli
        default: allowed = a.can.usage
        }
        return allowed ? nil : Texts.canWhy(w, appLabel: model.appLabel(a.tool), tool: a.tool)
    }
}

/// 没有查用量接口的工具（Antigravity）：用量这一节只说去哪里看，不画额度、没有刷新按钮。
struct NoUsageSection: View {
    let account: Account

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader("用量") { EmptyView() }
            Text(Texts.noUsage(account.tool))
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// “终端”按钮：点一下用这个账号在最近用的文件夹里开一个命令行会话；下拉里换文件夹、换终端。
struct SessionButton: View {
    @Environment(AppModel.self) private var model
    let account: Account
    var prominent = false
    /// 为什么现在不能用（cca 给的原因）。
    var unavailable: String?

    var body: some View {
        let a = account
        let recent = model.sessionDirectories
        Menu {
            if !recent.isEmpty {
                Section("在这个文件夹里打开") {
                    ForEach(recent, id: \.self) { dir in
                        Button(model.displayPath(dir)) { model.openSession(a.ref, directory: dir) }
                    }
                }
            }
            Button("选择其他文件夹…") { model.chooseSessionDirectory(a.ref) }
            Divider()
            Menu("用哪个终端打开") { TerminalMenuItems() }
        } label: {
            Label(title(recent.first), systemImage: "terminal")
                .lineLimit(1)
        } primaryAction: {
            model.openSession(a.ref)
        }
        .glassButton(prominent: prominent)
        .disabled(!a.can.cli || model.activity(a.ref) != nil)
        .help(unavailable ?? helpText(a, directory: recent.first))
    }

    /// 按钮上的字：带上最近用的文件夹的名字，太长时截短（完整路径在提示里）。
    private func title(_ directory: String?) -> String {
        guard let directory else { return "终端" }
        let name = (directory as NSString).lastPathComponent
        return "终端 · " + (name.count > 20 ? name.prefix(19) + "…" : name)
    }

    private func helpText(_ a: Account, directory: String?) -> String {
        let cli = Texts.cliName(a.tool)
        let t = model.terminalChoice
        guard let directory else { return "先选一个文件夹，再用 \(t.name) 在里面启动这个账号的 \(cli)" }
        guard t.isAutomatic else {
            return "打开 \(t.name) 并复制命令：到它的本地终端里粘贴运行，在 \(model.displayPath(directory)) 里启动这个账号的 \(cli)"
        }
        return "用 \(t.name) 在 \(model.displayPath(directory)) 里启动这个账号的 \(cli)"
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
