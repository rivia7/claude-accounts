import SwiftUI

/// 当前管理 ChatGPT 桌面客户端。版本、安装资格和原因均来自 cca，不在界面重算校验规则。
struct ClientUpdatesPage: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                ViewThatFits(in: .horizontal) {
                    HStack(alignment: .center) { heading; Spacer(minLength: 20); checkButton }
                    VStack(alignment: .leading, spacing: 14) { heading; checkButton }
                }
                Card("自动检查", systemImage: "arrow.clockwise.circle") {
                    Toggle(isOn: Binding(get: { model.clientUpdates?.mode == "auto" },
                                         set: { model.setClientUpdateAutomatic($0) })) {
                        VStack(alignment: .leading, spacing: 5) {
                            Text("自动检查并准备更新")
                            Text("cca 运行期间会检查更新。安装前会询问，客户端仍在运行时不会替换。")
                                .font(.callout).foregroundStyle(.secondary)
                        }
                    }
                    .toggleStyle(.switch)
                    .disabled(model.clientUpdates == nil || model.clientUpdatesBusy)
                    if model.hello?.devMode == true || model.isFixtureMode {
                        Text(model.isFixtureMode ? "演示数据：不会自动检查或安装。" : "开发模式：只在手动操作时检查。")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
                if let error = model.clientUpdatesError {
                    Banner(systemImage: "exclamationmark.triangle", tint: .orange,
                           title: "没能刷新客户端更新状态", message: error) {
                        Button("重新读取") { model.refreshClientUpdates() }
                            .buttonStyle(.bordered).disabled(model.clientUpdatesBusy)
                    }
                }
                ForEach(Array((model.clientUpdates?.clients ?? []).enumerated()), id: \.offset) { _, entry in
                    ClientUpdateCard(entry: entry)
                }
                if model.clientUpdates?.clients.isEmpty == true {
                    ContentUnavailableView("暂无客户端更新状态", systemImage: "shippingbox",
                                           description: Text("点“立即检查”获取当前客户端和可用版本。"))
                }
                Text("安装前请保存工作并退出全部 ChatGPT 实例。更新仅替换应用程序，账号资料保留。")
                    .font(.caption).foregroundStyle(.secondary)
            }
            .padding(.top, 24).padding(.bottom, 36).pageColumn()
        }
        .navigationTitle("客户端更新")
    }

    private var heading: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                Text("客户端更新").font(.system(size: 24, weight: .semibold))
                if model.clientUpdatesBusy { ProgressView().controlSize(.small) }
            }
            Text("检查这台 Mac 上的 ChatGPT 桌面客户端，并选择何时安装。")
                .font(.callout).foregroundStyle(.secondary)
        }
    }

    private var checkButton: some View {
        Button { model.checkClientUpdates(manual: true) } label: {
            Label(model.clientUpdateActivity == .check ? "正在检查…" : "立即检查", systemImage: "arrow.clockwise")
        }
        .buttonStyle(.glass).disabled(model.clientUpdatesBusy)
    }
}

private struct ClientUpdateCard: View {
    @Environment(AppModel.self) private var model
    let entry: ClientUpdateEntry

    var body: some View {
        Card(nonEmpty(entry.label) ?? entry.id, systemImage: "app.badge") {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top, spacing: 24) {
                    version("当前版本", entry.installedVersion)
                    Spacer(minLength: 8)
                    version(entry.stagedVersion == nil ? "可用版本" : "已准备版本",
                            entry.stagedVersion ?? entry.availableVersion)
                }
                VStack(alignment: .leading, spacing: 6) {
                    Text(entry.statusText).font(.headline).foregroundStyle(statusColor)
                    Text(entry.detail).font(.callout).foregroundStyle(.secondary).textSelection(.enabled)
                    if let build = nonEmpty(entry.packageBuild) {
                        Text("软件包 \(entry.packageVersion ?? "") · 构建 \(build)")
                            .font(.caption).foregroundStyle(.secondary).monospacedDigit()
                    }
                    if let path = nonEmpty(entry.path) {
                        Text(path).font(.caption).foregroundStyle(.tertiary).textSelection(.enabled)
                    }
                    if let checked = entry.checkedEpoch {
                        Text("上次检查：\(Date(timeIntervalSince1970: checked).formatted(date: .abbreviated, time: .shortened))")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
                HStack(spacing: 12) {
                    Button { model.installClientUpdate(entry) } label: {
                        Label(model.clientUpdateActivity == .install ? "正在安装…" : "安装更新…", systemImage: "arrow.down.to.line")
                    }
                    .buttonStyle(.glassProminent)
                    .disabled(!entry.readyToInstall || model.clientUpdatesBusy)
                    if entry.status == "needs_recheck" {
                        Button("重新检查") { model.checkClientUpdates(manual: true) }
                            .buttonStyle(.glass).disabled(model.clientUpdatesBusy)
                    }
                }
            }
        }
    }

    private func version(_ title: String, _ value: String?) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title).font(.caption).foregroundStyle(.secondary)
            Text(nonEmpty(value) ?? "—").font(.title2.weight(.medium)).monospacedDigit().textSelection(.enabled)
        }
    }

    private var statusColor: Color {
        switch entry.status {
        case "error": return .red
        case "blocked", "unverified", "needs_recheck", "unsupported": return .orange
        case "busy": return .secondary
        default: return .primary
        }
    }
}
