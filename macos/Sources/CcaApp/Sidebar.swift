import SwiftUI

/// 侧栏：工具切换（Claude / ChatGPT），最上面是节点分流（两种工具共用的），下面是这个工具的账号。
struct Sidebar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        List(selection: $model.selection) {
            Section {
                RoutingRow()
                    .tag(SidebarItem.routing)
            }
            Section(Texts.product(model.tool) + " 账号") {
                let accounts = model.accounts(model.tool)
                if accounts.isEmpty, model.state != nil {
                    Text(model.tool == .codex ? "还没有 ChatGPT 账号" : "还没有账号")
                        .foregroundStyle(.secondary)
                        .font(.callout)
                }
                ForEach(accounts) { a in
                    AccountRow(account: a)
                        .tag(SidebarItem.account(a.ref))
                        .contextMenu {
                            Button("改别名…") { model.beginRename(a.ref) }
                            if !a.isAPI {
                                Divider()
                                Button("打开 \(model.appLabel(a.tool))") { model.openApp(a.ref) }
                                    .disabled(!a.can.app || model.activity(a.ref) != nil)
                                Button("打开网页版") { model.openWeb(a.ref) }
                                    .disabled(!a.can.web || model.activity(a.ref) != nil)
                            }
                            Button("检测出口") { model.checkExit(a.ref) }
                                .disabled(!a.can.check || model.activity(a.ref) != nil)
                        }
                }
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

/// 节点分流那一行：品牌标记（App 图标的缩小版）、现在走哪边，有问题时右边说一句。
struct RoutingRow: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 10) {
            BrandMark(size: 30)
            VStack(alignment: .leading, spacing: 2) {
                Text("节点分流").font(.body.weight(.medium))
                Text(subtitle).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer(minLength: 4)
            if let issue {
                Tag(text: issue.0, color: issue.1)
            }
        }
        .padding(.vertical, 3)
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

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if model.phase == .ready, !model.codexEnabled {
                Button {
                    model.runInTerminal(.addAccount(prefix: ToolID.codex.defaultPrefix, product: "ChatGPT"))
                } label: {
                    Label("启用 ChatGPT 账号", systemImage: "plus.circle")
                }
                .buttonStyle(.borderless)
                .help("在终端里运行 cca codex add")
            }
            HStack(spacing: 6) {
                if model.stateLoading {
                    ProgressView().controlSize(.mini)
                }
                Text(statusText).font(.caption2).foregroundStyle(.tertiary).lineLimit(1)
                Spacer(minLength: 4)
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

/// 侧栏的一行：头像（App 开着时右下角有绿点）、名字、第二行（登录有问题时先说、套餐、节点），右边两行小用量。
struct AccountRow: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        let a = account
        HStack(spacing: 10) {
            BadgeAvatar(badge: a.badge, isAPI: a.isAPI, size: 30)
                .overlay(alignment: .bottomTrailing) {
                    if a.appRunning {
                        RunningDot(size: 10)
                            .offset(x: 1, y: 1)
                            .help("\(model.appLabel(a.tool)) 开着")
                    }
                }
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Text(a.displayName).font(.body.weight(.medium)).lineLimit(1)
                    if a.pinned {
                        Image(systemName: "lock.fill")
                            .font(.system(size: 9))
                            .foregroundStyle(.tertiary)
                            .help("维护者锁定了节点")
                    }
                }
                HStack(spacing: 5) {
                    if let c = a.loginIssueColor {
                        Circle().fill(c).frame(width: 6, height: 6)
                    }
                    Text(a.subtitle).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer(minLength: 4)
            if let u = model.usage(for: a), u.source != .none {
                UsageStack(usage: u)
            } else if model.isUsageRefreshing(a.ref), a.can.usage {
                ProgressView().controlSize(.mini)
            }
        }
        .padding(.vertical, 3)
        .accessibilityElement(children: .combine)
    }
}
