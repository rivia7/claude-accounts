import SwiftUI

/// 账号页：每个工具一张白色的列表（Claude、ChatGPT），一行一个账号——头像、名字和登录状态，右边是 5h / 周的用量、节点、【打开】。
/// 点一行在它下面展开详情（用量、线路与出口、网页版等操作），再点一次收起；同时只展开一个（就是 AppModel 的 selection）。
struct AccountsPage: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                ForEach(model.visibleTools, id: \.self) { t in
                    ToolSection(tool: t)
                }
                if model.phase == .ready, !model.codexEnabled {
                    EnableCodex()
                }
                FooterStatus()
            }
            .padding(.horizontal, 24)
            .padding(.top, 14)
            .padding(.bottom, 28)
            .frame(maxWidth: 1000)
            .frame(maxWidth: .infinity)
        }
    }
}

/// 一个工具的账号：小标题（右边是添加、编辑账号表），下面一张列表。
struct ToolSection: View {
    @Environment(AppModel.self) private var model
    let tool: ToolID

    var body: some View {
        let accounts = model.accounts(tool)
        VStack(alignment: .leading, spacing: 6) {
            Caption(Texts.product(tool) + " 账号") {
                Button {
                    model.runInTerminal(.addAccount(prefix: tool == .claude ? [] : model.prefix(.codex), product: Texts.product(tool)))
                } label: {
                    Label("添加", systemImage: "plus")
                }
                .buttonStyle(.quiet)
                .help("在终端里添加一个 \(Texts.product(tool)) 账号")
                Button("编辑账号表…") { model.runInTerminal(.edit(prefix: model.prefix(tool))) }
                    .buttonStyle(.quiet)
                    .help("在终端里用编辑器改 \(Texts.product(tool)) 的账号表")
            }
            if accounts.isEmpty {
                Text(model.state == nil ? "正在读取…" : "还没有 \(Texts.product(tool)) 账号。点上面的“添加”，在终端里登录一个。")
                    .font(.system(size: 12.5))
                    .foregroundStyle(Theme.muted)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 14)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .card()
            } else {
                VStack(spacing: 0) {
                    ForEach(Array(accounts.enumerated()), id: \.element.id) { i, a in
                        if i > 0 { Hairline() }
                        AccountListRow(account: a)
                        if model.selection == .account(a.ref) {
                            Hairline()
                            AccountDetailPanel(account: a)
                                .transition(.opacity.combined(with: .move(edge: .top)))
                        }
                    }
                }
                .card()
            }
        }
    }
}

extension AppModel {
    /// 展开 / 收起一个账号的详情（同时只展开一个）。换工具会改 selection，所以先记下原来是不是展开着。
    func toggleDetail(_ ref: AccountRef) {
        let wasOpen = selection == .account(ref)
        if tool != ref.tool { tool = ref.tool }
        selection = wasOpen ? nil : .account(ref)
    }
}

/// 列表里的一行：头像（App 开着时右下角有绿点）、名字和登录状态；右边是用量、节点、【打开】、展开的箭头。
struct AccountListRow: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        let a = account
        let open = model.selection == .account(a.ref)
        let live = model.liveSlot(for: a.ref)
        HoverRow(active: open) {
            HStack(spacing: 12) {
                BadgeAvatar(badge: a.badge, isAPI: a.isAPI, size: 30)
                    .overlay(alignment: .bottomTrailing) {
                        if a.appRunning {
                            RunningDot(size: 10).offset(x: 2, y: 2)
                                .help("\(model.appLabel(a.tool)) 开着")
                        }
                    }
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 5) {
                        Text(a.displayName)
                            .font(.system(size: 13.5, weight: .medium))
                            .foregroundStyle(Theme.fg)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        if a.isLast {
                            Image(systemName: "terminal")
                                .font(.system(size: 9.5, weight: .semibold))
                                .foregroundStyle(Theme.faint)
                                .help("上次在命令行里用的")
                        }
                    }
                    HStack(spacing: 5) {
                        StatusDot(tone: Palette.login(a.login), size: 6)
                        Text(subline(a)).foregroundStyle(Theme.muted).lineLimit(1)
                    }
                    .font(.system(size: 11.5))
                }
                Spacer(minLength: 10)
                if let u = model.usage(for: a), u.source != .none {
                    RowQuota(usage: u)
                } else if model.isUsageRefreshing(a.ref), a.can.usage {
                    ProgressView().controlSize(.mini)
                }
                FieldPill(text: a.node ?? "直连", symbol: a.pinned ? "lock.fill" : nil, edge: edge(live))
                    .help(nodeHelp(a, live))
                actionArea(a)
                Image(systemName: "chevron.down")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(Theme.faint)
                    .rotationEffect(.degrees(open ? 180 : 0))
                    .frame(width: 12)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .contentShape(Rectangle())
            .onTapGesture {
                withAnimation(.snappy(duration: 0.28)) { model.toggleDetail(a.ref) }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityAddTraits(.isButton)
        .accessibilityHint(open ? "收起详情" : "展开详情")
    }

    /// 名字下面那行：登录状态 · 套餐（API 账号：接口地址）；App 开着时再说一句。
    private func subline(_ a: Account) -> String {
        var s = a.statusLine
        if a.appRunning { s += " · \(model.appLabel(a.tool)) 开着" }
        return s
    }

    private func edge(_ live: LiveSlot?) -> Color? {
        switch live?.state {
        case .drift?: return Theme.drift
        case .closed?: return Theme.red
        default: return nil
        }
    }

    private func nodeHelp(_ a: Account, _ live: LiveSlot?) -> String {
        guard let node = a.node else { return "不经节点，直连" }
        var s = "节点 \(node)" + (a.port.map { "，端口 \($0)" } ?? "")
        if a.pinned { s += "（维护者锁定）" }
        switch live?.state {
        case .drift?: s += "；路由器上选中的是 \(live?.selected ?? "—")"
        case .closed?: s += "；端口连不上"
        default: break
        }
        return s
    }

    /// 右边的按钮：没登录时是“登录…”，官方账号是“打开”，第三方 API 账号没有（占个位，几行对齐）。
    @ViewBuilder
    private func actionArea(_ a: Account) -> some View {
        let busy = model.activity(a.ref) != nil
        Group {
            if busy {
                ProgressView().controlSize(.small)
            } else if a.login == .loggedOut || a.login == .emailMismatch {
                Button("登录") { model.runInTerminal(model.loginHandoff(a)) }
                    .buttonStyle(.action)
                    .help((a.login == .emailMismatch ? "登录的不是这个账号，重新登录：" : "") + "在终端里运行 \(model.loginHandoff(a).display)")
            } else if a.isAPI {
                Color.clear.frame(width: 1, height: 1)
            } else {
                Button("打开") { model.openApp(a.ref) }
                    .buttonStyle(.action)
                    .disabled(!a.can.app)
                    .help(a.can.app ? "用这个账号的出口和独立的资料目录打开 \(model.appLabel(a.tool))"
                          : (a.canWhy["app"].map { Texts.canWhy($0, appLabel: model.appLabel(a.tool), tool: a.tool) } ?? "不能打开"))
                    .accessibilityLabel("打开 \(a.displayName) 的 \(model.appLabel(a.tool))")
            }
        }
        .frame(width: 66, alignment: .trailing)
    }
}

/// 展开的详情（magpie 的展开编辑区，次一级的底色）：一行身份和操作，下面是用量、线路与出口。
struct AccountDetailPanel: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        let a = account
        VStack(alignment: .leading, spacing: 18) {
            header(a)
            if !a.isAPI { UsageBlock(account: a) }
            RouteBlock(account: a)
        }
        .padding(.horizontal, 18)
        .padding(.top, 14)
        .padding(.bottom, 18)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.card2)
    }

    /// 邮箱、套餐、上次在命令行里用的；右边是打开 App、网页版（第三方 API 账号是接口和模型）。正在打开、检测时多一行进度。
    private func header(_ a: Account) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .center, spacing: 10) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(identity(a))
                        .font(.system(size: 12.5))
                        .foregroundStyle(Theme.fg2)
                        .textSelection(.enabled)
                        .lineLimit(1)
                    if let why = disabledReasons(a) {
                        Text(why).font(.system(size: 11.5)).foregroundStyle(Theme.muted)
                    }
                }
                Spacer(minLength: 8)
                if !a.isAPI {
                    Button {
                        model.openWeb(a.ref)
                    } label: {
                        Label("网页版", systemImage: "safari")
                    }
                    .buttonStyle(.action)
                    .disabled(!a.can.web || model.activity(a.ref) != nil)
                    .help(reason("web", a) ?? "在这个账号自己的 Chrome 资料里打开网页版，走账号的出口")
                    Button {
                        model.openApp(a.ref)
                    } label: {
                        Label("打开 \(model.appLabel(a.tool))", systemImage: "macwindow")
                    }
                    .buttonStyle(.primary)
                    .disabled(!a.can.app || model.activity(a.ref) != nil)
                    .help(reason("app", a) ?? "用这个账号的出口和独立的资料目录打开 \(model.appLabel(a.tool))")
                }
            }
            if let busy = model.activity(a.ref) {
                HStack(spacing: 7) {
                    ProgressView().controlSize(.mini)
                    Text(progressText(busy)).font(.system(size: 12)).foregroundStyle(Theme.fg2)
                }
                .accessibilityElement(children: .combine)
            }
        }
    }

    private func identity(_ a: Account) -> String {
        if a.isAPI {
            return [a.api?.host, a.api?.model.map { "模型 \($0)" }].compactMap { nonEmpty($0) }.joined(separator: " · ")
        }
        var parts: [String] = []
        if let e = nonEmpty(a.email) { parts.append(e) }
        if let p = nonEmpty(a.plan) { parts.append(p) }
        if a.login == .emailMismatch, let l = nonEmpty(a.loginEmail) { parts.append("现在登录的是 \(l)") }
        if a.isLast { parts.append("上次在命令行里用的") }
        return parts.joined(separator: " · ")
    }

    /// 按钮为什么不能用（cca 给的原因码）。
    private func disabledReasons(_ a: Account) -> String? {
        if a.isAPI { return nil }
        var out: [String] = []
        let label = model.appLabel(a.tool)
        if !a.can.app, let w = a.canWhy["app"] { out.append("打开 App：" + Texts.canWhy(w, appLabel: label, tool: a.tool)) }
        if !a.can.web, let w = a.canWhy["web"] { out.append("网页版：" + Texts.canWhy(w, appLabel: label, tool: a.tool)) }
        return out.isEmpty ? nil : out.joined(separator: "；")
    }

    private func reason(_ action: String, _ a: Account) -> String? {
        guard let w = a.canWhy[action] else { return nil }
        let allowed = action == "app" ? a.can.app : a.can.web
        return allowed ? nil : Texts.canWhy(w, appLabel: model.appLabel(a.tool), tool: a.tool)
    }

    private func progressText(_ act: Activity) -> String {
        let appLabel = model.appLabel(account.tool)
        if let s = act.step { return Texts.step(s, appLabel: appLabel) }
        switch act.kind {
        case .app: return "打开 \(appLabel)…"
        case .web: return "打开网页版…"
        case .check: return "检测出口…"
        case .sync: return "同步路由器…"
        }
    }
}

/// 还没用 cca codex 时：一张卡片说一句，按钮在终端里添加第一个 ChatGPT 账号。
struct EnableCodex: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Caption("ChatGPT 账号")
            HStack(spacing: 12) {
                Text("还没有用 cca 管 ChatGPT（Codex）账号。添加第一个之后，这里会列出它们。")
                    .font(.system(size: 12.5))
                    .foregroundStyle(Theme.muted)
                Spacer(minLength: 8)
                Button("添加 ChatGPT 账号…") {
                    model.runInTerminal(.addAccount(prefix: ToolID.codex.defaultPrefix, product: "ChatGPT"))
                }
                .buttonStyle(.action)
                .help("在终端里运行 cca codex add")
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            .card()
        }
    }
}

/// 页面最下面的一行小字：cca 的版本、开发模式、演示数据。
struct FooterStatus: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 6) {
            if model.stateLoading { ProgressView().controlSize(.mini) }
            Text(statusText).font(.system(size: 11)).foregroundStyle(Theme.faint)
        }
        .padding(.horizontal, 4)
    }

    private var statusText: String {
        var parts: [String] = []
        if let v = model.hello?.ccaVersion { parts.append("cca \(v)") }
        if model.hello?.devMode == true { parts.append("开发模式") }
        if model.isFixtureMode { parts.append("演示数据") }
        return parts.joined(separator: " · ")
    }
}
