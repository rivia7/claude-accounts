import SwiftUI

/// 侧栏：工具切换（Claude / ChatGPT）、账号列表、节点分流。
struct Sidebar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        List(selection: $model.selection) {
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
                }
            }
            Section("路由") {
                Label {
                    HStack {
                        Text("节点分流")
                        Spacer()
                        if model.state?.routing?.sync == .pending {
                            Image(systemName: "exclamationmark.circle.fill")
                                .foregroundStyle(.orange)
                                .help("路由器待同步")
                                .accessibilityLabel("路由器待同步")
                        } else if model.live?.hasDrift == true {
                            Image(systemName: "exclamationmark.circle.fill")
                                .foregroundStyle(.orange)
                                .help("有槽位选错了节点")
                                .accessibilityLabel("有槽位选错了节点")
                        }
                    }
                } icon: {
                    Image(systemName: "point.3.filled.connected.trianglepath.dotted")
                }
                .tag(SidebarItem.routing)
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
                Text(statusText).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
            }
        }
        .padding(.horizontal, 14)
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

/// 侧栏的一行：头像、名字、套餐、登录状态、两个小用量条、锁定、节点、App 开着的绿点。
struct AccountRow: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        let a = account
        HStack(spacing: 10) {
            BadgeAvatar(badge: a.badge, isAPI: a.isAPI, size: 30)
                .overlay(alignment: .bottomTrailing) {
                    if a.appRunning {
                        Circle().fill(.green)
                            .frame(width: 9, height: 9)
                            .overlay(Circle().stroke(.background, lineWidth: 1.5))
                            .help("\(model.appLabel(a.tool)) 开着")
                    }
                }
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 4) {
                    Text(a.displayName).font(.body.weight(.medium)).lineLimit(1)
                    if a.pinned {
                        Image(systemName: "lock.fill")
                            .font(.system(size: 9))
                            .foregroundStyle(.secondary)
                            .help("维护者锁定了节点")
                    }
                }
                HStack(spacing: 5) {
                    LoginDot(state: a.login)
                    Text(a.subtitle).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer(minLength: 4)
            if let u = model.usage(for: a), u.source != .none {
                VStack(alignment: .trailing, spacing: 3) {
                    MiniUsage(label: "5h", window: u.fiveHour, stale: u.stale)
                    MiniUsage(label: "周", window: u.sevenDay, stale: u.stale)
                }
            } else if model.isUsageRefreshing(a.ref), a.can.usage {
                ProgressView().controlSize(.mini)
            }
        }
        .padding(.vertical, 3)
        .accessibilityElement(children: .combine)
    }
}
