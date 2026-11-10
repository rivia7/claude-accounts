import SwiftUI

/// 管理这台 Mac 上的 Claude、ChatGPT 桌面客户端和终端里的 Claude Code、Codex（没装的不显示）。版本、安装资格和原因均来自 cca，不在界面重算校验规则。
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
                            Text("自动检查更新")
                            Text("定期看有没有新版本，桌面 App 的会先下载好。安装前一定先问你；客户端还开着时不会替换。")
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
                           title: "没能读取客户端的更新状态", message: error) {
                        Button("重新读取") { model.refreshClientUpdates() }
                            .buttonStyle(.borderedProminent).disabled(model.clientUpdatesBusy)
                    }
                }
                ForEach(Array(installed.enumerated()), id: \.offset) { _, entry in
                    ClientUpdateCard(entry: entry)
                }
                if model.clientUpdates != nil && installed.isEmpty {
                    ContentUnavailableView("这台 Mac 上还没有客户端", systemImage: "shippingbox",
                                           description: Text("装好 Claude、ChatGPT 桌面 App，或者 Claude Code、Codex 命令后，这里会显示它们的版本和更新。"))
                }
                Text("安装桌面 App 的更新前，先保存工作、退出它的全部窗口；更新只换程序，账号资料不动。终端里的 Claude Code、Codex 更新后，开着的会话还是旧版本，新开的才是新版本。")
                    .font(.caption).foregroundStyle(.secondary)
            }
            .padding(.top, 24).padding(.bottom, 36).pageColumn()
        }
        .navigationTitle("客户端更新")
    }

    /// 这台 Mac 上装了的客户端。
    private var installed: [ClientUpdateEntry] {
        (model.clientUpdates?.clients ?? []).filter { $0.status != "missing" }
    }

    private var heading: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                Text("客户端更新").font(.system(size: 24, weight: .semibold))
                if model.clientUpdatesBusy { ProgressView().controlSize(.small) }
            }
            Text("这台 Mac 上的桌面 App，和终端里的 Claude Code、Codex。有新版本时由你决定什么时候安装。")
                .font(.callout).foregroundStyle(.secondary)
        }
    }

    private var checkButton: some View {
        SectionAction(model.clientUpdateActivity == .check ? "正在检查…" : "立即检查", systemImage: "arrow.clockwise") {
            model.checkClientUpdates(manual: true)
        }
        .disabled(model.clientUpdatesBusy)
    }
}

private struct ClientUpdateCard: View {
    @Environment(AppModel.self) private var model
    let entry: ClientUpdateEntry

    var body: some View {
        Card(nonEmpty(entry.label) ?? entry.id, systemImage: entry.isCLI ? "terminal" : "macwindow") {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top, spacing: 24) {
                    version("当前版本", entry.installedVersion)
                    Spacer(minLength: 8)
                    version(entry.stagedVersion == nil ? "最新版本" : "已下载的版本",
                            entry.stagedVersion ?? entry.availableVersion)
                }
                VStack(alignment: .leading, spacing: 6) {
                    Text(entry.statusText).font(.headline).foregroundStyle(statusColor)
                    Text(entry.detail).font(.callout).foregroundStyle(.secondary).textSelection(.enabled)
                    if let build = nonEmpty(entry.packageBuild) {
                        Text("安装包 \(entry.packageVersion ?? "") · 构建 \(build)")
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
                // 能装的才有「安装更新…」（已是最新、npm / Homebrew 装的不能在这里装，原因在上面那句话里）；要重新检查时才有「重新检查」
                if entry.readyToInstall || entry.status == "needs_recheck" {
                    GlassRow(spacing: 10) {
                        if entry.readyToInstall {
                            SectionAction(model.clientUpdateActivity == .install ? "正在安装…" : "安装更新…", systemImage: "arrow.down.to.line",
                                          prominent: true) { model.installClientUpdate(entry) }
                                .disabled(model.clientUpdatesBusy)
                        }
                        if entry.status == "needs_recheck" {
                            SectionAction("重新检查", systemImage: "arrow.clockwise") { model.checkClientUpdates(manual: true) }
                                .disabled(model.clientUpdatesBusy)
                        }
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
