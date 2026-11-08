import SwiftUI

/// 侧栏：工具切换（Claude / ChatGPT），最上面是办公室（首页，所有账号一眼看全）和节点分流（两种工具共用的），
/// 下面是这个工具的账号（花名册）。
struct Sidebar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        List(selection: $model.selection) {
            Section {
                OfficeRow()
                    .tag(SidebarItem.office)
                RoutingRow()
                    .tag(SidebarItem.routing)
                HStack(spacing: 10) {
                    Image(systemName: "arrow.down.app").font(.title3).frame(width: 30, height: 30)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("客户端更新").font(.body.weight(.medium))
                        Text("桌面客户端和终端工具").font(.caption).foregroundStyle(.plainSecondary)
                    }
                }
                .sidebarRowPadding()
                .tag(SidebarItem.clientUpdates)
            }
            let groups = model.accountGroups(model.tool)
            ForEach(groups) { g in
                AccountSection(group: g, showsAdd: g.id == groups.first?.id)
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .top, spacing: 0) {
            if model.visibleTools.count > 1 {
                Picker("工具", selection: $model.tool) {
                    ForEach(model.visibleTools, id: \.self) { t in
                        Text(Texts.product(t)).tag(t)
                    }
                }
                .labelsHidden()
                .toolPickerStyle()
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            SidebarFooter()
        }
        .frame(minWidth: 250)
    }
}

/// 侧栏的一组账号：上下拖动调整顺序（只影响 App 里的显示），标题上有添加账号的按钮。
private struct AccountSection: View {
    @Environment(AppModel.self) private var model
    let group: AccountGroup
    let showsAdd: Bool

    var body: some View {
        Section {
            if group.accounts.isEmpty, model.state != nil {
                Text("还没有 \(Texts.product(model.tool)) 账号")
                    .foregroundStyle(.plainSecondary)
                    .font(.callout)
            }
            ForEach(group.accounts) { a in
                AccountRow(account: a)
                    .tag(SidebarItem.account(a.ref))
                    .contextMenu { AccountMenu(account: a) }
            }
            .onMove { model.moveAccounts(group.department, shown: group.accounts, from: $0, to: $1) }
        } header: {
            HStack(spacing: 4) {
                Text(Texts.department(group.department)).foregroundStyle(PlainSecondary(system: .tertiary))
                Spacer(minLength: 4)
                if showsAdd { AddAccountMenu() }
            }
        }
    }
}

/// 添加账号：这个工具的官方账号，或者第三方 API 账号。
private struct AddAccountMenu: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let t = model.tool
        let official = model.addHandoff(t == .codex ? .chatgpt : .claude)
        let api = Handoff.addAPI(prefix: model.prefix(t))
        Menu {
            Button(official.title + "…") { model.run(official) }
            Button(api.title + "…") { model.run(api) }
        } label: {
            Image(systemName: "plus")
                .font(.system(size: 12, weight: .semibold))
                .frame(width: 20, height: 18)
                .contentShape(Rectangle())
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
        .foregroundStyle(.plainSecondary)
        .disabled(model.phase != .ready)
        .help("添加账号")
        .accessibilityLabel("添加账号")
    }
}

/// 账号的右键菜单（侧栏的账号、办公室的工位共用）。
struct AccountMenu: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        let a = account
        let busy = model.activity(a.ref) != nil
        Button("改别名…") { model.beginRename(a.ref) }
        Divider()
        if !a.isAPI {
            Button("打开 \(model.appLabel(a.tool))") { model.openApp(a.ref) }
                .disabled(!a.can.app || busy)
            Button("打开网页版") { model.openWeb(a.ref) }
                .disabled(!a.can.web || busy)
        }
        Button("在终端里打开") { model.openSession(a.ref) }
            .disabled(!a.can.cli || busy)
        Button("检测出口") { model.checkExit(a.ref) }
            .disabled(!a.can.check || busy)
        Divider()
        AccountManageItems(account: a, busy: busy)
    }
}

/// 管理账号的菜单项（右键菜单、账号页头部的“更多”共用）。
struct AccountManageItems: View {
    @Environment(AppModel.self) private var model
    let account: Account
    let busy: Bool

    var body: some View {
        let a = account
        let top = model.isOnTop(a)
        Button(top ? "取消置顶" : "置顶") { model.setOnTop(a, !top) }
        if a.isAPI {
            Button("更换 API key…") { model.run(.apiKey(prefix: model.prefix(a.tool), key: a.key, name: a.displayName)) }
                .disabled(busy)
        } else {
            Button(a.login == .loggedOut ? "登录…" : "重新登录…") { model.relogin(a) }
                .disabled(busy)
            if a.can.policyLock {
                Button("请求维护者锁定节点") { model.requestPolicyLock(a) }
            }
        }
        Button("删除账号…", role: .destructive) { model.run(.remove(prefix: model.prefix(a.tool), key: a.key, name: a.displayName)) }
            .disabled(busy)
    }
}

/// 总览那一行：总览的图标（OverviewMark）、几个账号几个在用，有要处理的时右边说一句。
struct OfficeRow: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        // “休息到几点”到点就醒：跟着时间重算
        TimelineView(.periodic(from: .now, by: 30)) { ctx in
            let census = model.office(now: ctx.date.timeIntervalSince1970).census
            HStack(spacing: 10) {
                OverviewMark(size: 22).frame(width: 30, height: 30)
                VStack(alignment: .leading, spacing: 2) {
                    Text("总览").font(.body.weight(.medium))
                    Text(model.state == nil ? "所有账号一眼看全" : Texts.officeSummary(census, brief: true))
                        .font(.caption).foregroundStyle(.plainSecondary).lineLimit(1)
                }
                Spacer(minLength: 4)
                if census.needsAttention > 0 {
                    Tag(text: "\(census.needsAttention) 个要处理", color: .orange)
                }
            }
            .sidebarRowPadding()
            .accessibilityElement(children: .combine)
        }
    }

}

/// 节点分流那一行：品牌标记（App 图标的缩小版）、现在走哪边，有问题时右边说一句。
struct RoutingRow: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 10) {
            BrandMark(size: 26).frame(width: 30, height: 30)
            VStack(alignment: .leading, spacing: 2) {
                Text("节点分流").font(.body.weight(.medium))
                Text(subtitle).font(.caption).foregroundStyle(.plainSecondary).lineLimit(1)
            }
            Spacer(minLength: 4)
            if let issue {
                Tag(text: issue.0, color: issue.1)
            }
        }
        .sidebarRowPadding()
        .accessibilityElement(children: .combine)
    }

    private var subtitle: String { model.routeSummary }
    private var issue: (String, Color)? { model.routeIssue }
}

extension AppModel {
    /// 走哪边（侧栏的节点分流、菜单栏面板的第二行）：读到 live 时按它，否则按设置。
    var routeSummary: String {
        if let l = live {
            switch l.route {
            case .router: return "路由器" + (l.router?.backend.map { " · \($0.rawValue)" } ?? "")
            case .local: return nonEmpty(l.local?.name) ?? "本机 Clash"
            default: break
            }
        }
        if routerConfigured { return "路由器" + (state?.config?.router?.host.map { " \($0)" } ?? "") }
        if localConfigured { return nonEmpty(state?.config?.local?.name) ?? "本机 Clash" }
        return "没配置路由器和本机 Clash"
    }

    /// 分流有问题时的一句话和颜色：待同步、有槽位连不上、选错节点。
    var routeIssue: (String, Color)? {
        if state?.routing?.sync == .pending { return ("待同步", .orange) }
        if let s = live?.summary {
            if s.closed > 0 { return ("\(s.closed) 个连不上", .red) }
            if s.drift > 0 { return ("\(s.drift) 个选错", .orange) }
        }
        return nil
    }
}

struct SidebarFooter: View {
    @Environment(AppModel.self) private var model
    @Environment(Guide.self) private var guide

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if model.phase == .ready, !model.codexEnabled {
                Button {
                    model.run(.addAccount(prefix: ToolID.codex.defaultPrefix, product: "ChatGPT"))
                } label: {
                    Label("启用 ChatGPT 账号", systemImage: "plus.circle")
                }
                .buttonStyle(.borderless)
            }
            HStack(spacing: 6) {
                if model.stateLoading || model.updateChecking {
                    ProgressView().controlSize(.mini)
                }
                // 版本号：点一下检查更新；有新版本时后面带一句（顶上的提示条里能更新）
                Button {
                    model.checkForUpdates(manual: true)
                } label: {
                    HStack(spacing: 4) {
                        Text(statusText).foregroundStyle(PlainSecondary(system: .tertiary))
                        if let v = model.availableVersion {
                            Text("· 有新版本 \(v)").foregroundStyle(.tint)
                        }
                    }
                    .font(.caption2)
                    .lineLimit(1)
                }
                .buttonStyle(.plain)
                .disabled(model.phase != .ready || model.updateChecking)
                .help("检查 cca 更新")
                .accessibilityLabel(statusText + "，检查 cca 更新")
                Spacer(minLength: 4)
                Button {
                    guide.openTutorial()
                } label: {
                    Label("新手教程", systemImage: "book")
                        .font(.caption)
                }
                .buttonStyle(.borderless)
                .help("重新看一遍新手教程（⇧⌘/）")
                AppearanceMenu()
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var statusText: String {
        var parts: [String] = []
        if let v = model.hello?.ccaVersion { parts.append("cca \(v)") }
        if model.hello?.devMode == true { parts.append("开发模式") }
        if model.isFixtureMode { parts.append("演示数据") }
        return parts.joined(separator: " · ")
    }
}

extension View {
    /// 侧栏一行的边距。左边多留出最高档头像光晕的宽度：列表的一格只到头像的边，画到格子外面的会被裁掉；
    /// 每一行都留，各行的图标（宽 30）照样对齐。
    func sidebarRowPadding() -> some View {
        padding(.vertical, 3).padding(.leading, BadgeAvatar.haloRoom(30))
    }
}

/// 置顶的账号名字后面的小图钉（侧栏、总览的卡片）。
struct OnTopMark: View {
    var body: some View {
        Image(systemName: "pin.fill")
            .font(.system(size: 9))
            .foregroundStyle(.tertiary)
            .help("已置顶")
            .accessibilityLabel("已置顶")
    }
}

/// 侧栏的一行：头像（在用时右下角有绿点；最高档的套餐有光晕，只有这里有，在用时慢慢流动）、名字、
/// 第二行（登录有问题时先说、套餐徽章、节点），右边两行小用量。
struct AccountRow: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        let a = account
        HStack(spacing: 10) {
            AccountAvatar(account: a, size: 30, dot: 10, halo: true)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Text(a.displayName).font(.body.weight(.medium)).lineLimit(1)
                    if a.pinned {
                        Image(systemName: "lock.fill")
                            .font(.system(size: 9))
                            .foregroundStyle(.tertiary)
                            .help("维护者锁定了节点")
                    }
                    if model.isOnTop(a) { OnTopMark() }
                }
                AccountSubtitle(account: a, expired: model.loginExpired(a))
            }
            Spacer(minLength: 4)
            if let u = model.usage(for: a), u.source != .none {
                UsageStack(usage: u)
            } else if model.isUsageRefreshing(a.ref), a.can.usage {
                ProgressView().controlSize(.mini)
            }
        }
        .sidebarRowPadding()
        .accessibilityElement(children: .combine)
    }
}
