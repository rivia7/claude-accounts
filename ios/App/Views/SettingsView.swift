import CcaRemoteKit
import SwiftUI

/// 设置：外观、这台手机叫什么、配对过的电脑（取消配对）、通知（推送）、演示模式、帮助（新手教程、更新说明、重新显示提示卡片：Mac App 的
/// 「帮助」菜单）、关于。从这里打开的新手教程、更新说明弹在这一页上面
struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @State private var unpairing: Peer?
    private var push: Pusher { Pusher.shared }

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            Form {
                Section("外观") {
                    Picker("外观", selection: $model.appearance) {
                        ForEach(AppearanceMode.allCases) { Text($0.title).tag($0) }
                    }
                    .pickerStyle(.segmented)
                }
                Section {
                    TextField("名称", text: $model.deviceName)
                        .textInputAutocapitalization(.never)
                        .onSubmit {
                            if Texts.nonEmpty(model.deviceName) == nil { model.deviceName = AppModel.defaultName }
                        }
                } header: {
                    Text("这台手机的名称")
                } footer: {
                    Text("配对时发送给电脑，显示在电脑的「已配对的机器」中。修改后只影响之后的配对。")
                }
                Section {
                    if model.peers.isEmpty {
                        Text("暂无已配对的电脑").foregroundStyle(.secondary)
                    }
                    ForEach(model.peers) { p in
                        HStack(spacing: 10) {
                            Image(systemName: "desktopcomputer").foregroundStyle(.secondary).frame(width: 22)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(p.name).lineLimit(1)
                                Text(Texts.peerDetail(p, now: Date().timeIntervalSince1970) + " · 经 " + p.server.host)
                                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                                // 那台电脑上 cca 的版本（有新版本时写上）、让不让这台手机操作（电脑放上来的账号状态里有：旧版的 cca 不放，这一行就没有）
                                if let line = Texts.peerHost(model.status(p)) {
                                    Text(line).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                                }
                            }
                            Spacer(minLength: 6)
                            Button("取消配对") { unpairing = p }
                                .buttonStyle(.borderless)
                                .foregroundStyle(.red)
                                .font(.callout)
                        }
                    }
                    Button { model.showSettings = false; model.startPairing() } label: {
                        Label("和另一台电脑配对", systemImage: "qrcode.viewfinder")
                    }
                } header: {
                    Text("已配对的电脑")
                } footer: {
                    if let p = model.peers.first, let fp = model.backend.fingerprint(on: p.server) {
                        Text("这台手机的指纹：\(fp.prefix(19))…（可在电脑上运行 cca remote peers 核对）")
                    }
                }
                Section {
                    LabeledContent("推送", value: pushState)
                    if !model.isDemo {
                        Toggle("已关闭的会话也推送", isOn: Binding(get: { push.closedToo }, set: { push.closedToo = $0 }))
                        Toggle("额度用完、恢复时通知", isOn: Binding(get: { push.usageNotes }, set: { push.usageNotes = $0 }))
                    }
                    if push.allowed == false, !model.isDemo, let u = URL(string: UIApplication.openNotificationSettingsURLString) {
                        Button("在系统设置里打开通知") { UIApplication.shared.open(u) }
                    }
                } header: {
                    Text("通知")
                } footer: {
                    Text(pushFooter)
                }
                Section {
                    if model.isDemo {
                        Button("退出演示") { model.leaveDemo() }
                    } else {
                        Button("查看演示") { model.enterDemo(); model.showSettings = false }
                    }
                } footer: {
                    Text("演示模式中的电脑和会话都是模拟的，不会联网，也不影响这台手机上真实的配对。")
                }
                Section {
                    Button { model.guide.openTutorial() } label: { Label("新手教程", systemImage: "book") }
                    Button { model.guide.openWhatsNew() } label: { Label("更新说明", systemImage: "sparkles") }
                    Button {
                        model.guide.resetTips()
                        model.showToast("各页的提示卡片已重新显示", .success)
                    } label: {
                        Label("重新显示提示卡片", systemImage: "lightbulb")
                    }
                } header: {
                    Text("帮助")
                } footer: {
                    Text("提示卡片在总览、账号页、用量统计、折合金额的顶上，点「知道了」后不再显示。")
                }
                Section("关于") {
                    LabeledContent("版本", value: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?")
                    NavigationLink("开源许可") { LicensesView() }
                }
            }
            .navigationTitle("设置")
            .navigationBarTitleDisplayMode(.inline)
            .demoSubtitle()
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("完成") { model.showSettings = false }
                }
            }
            .confirmationDialog(unpairing.map { "取消和「\($0.name)」的配对？" } ?? "",
                                isPresented: Binding(get: { unpairing != nil }, set: { if !$0 { unpairing = nil } }),
                                titleVisibility: .visible) {
                Button("取消配对", role: .destructive) {
                    if let p = unpairing { model.unpair(p) }
                    unpairing = nil
                }
                Button("不取消", role: .cancel) { unpairing = nil }
            } message: {
                Text("取消后，这台手机将无法再接入那台电脑上的会话，正在接入的会断开；之后需要重新配对。")
            }
            .toastOverlay(bottom: 16)
        }
        .sheet(item: Binding(get: { model.guide.sheet }, set: { if $0 == nil { model.guide.dismissSheet() } })) { _ in
            GuideSheet()
        }
    }
}

extension SettingsView {
    /// 推送现在怎么样（一个词）
    private var pushState: String {
        if model.isDemo { return "演示模式不可用" }
        switch push.allowed {
        case false: return "已关闭"
        case nil: return "尚未授权"
        default: return push.token == nil ? (push.failure == nil ? "正在打开" : "未开启") : "已开启"
        }
    }

    private var pushFooter: String {
        var lines = ["锁屏或没打开 App 时，也能收到会话里的通知（做完了、在等你）。通知经电脑使用的服务器和 cca 的推送服务转发，"
            + "内容加密，只有这台手机能解密。"]
        if model.isDemo { return lines[0] }
        lines.append(push.closedToo
            ? "已关闭的会话也推送：已配对电脑上的会话都会推送，包括在 App 中关掉的。关闭此项后，只推送 App 中打开的会话。"
            : "只推送 App 中打开的会话（锁屏或切到其他 App 也算打开），关掉会话后不再推送。")
        if push.allowed == false { lines.append("通知已关闭：请在「设置」→「通知」→「cca」中打开。") }
        if let f = push.failure, push.token == nil { lines.append("获取 Apple 推送令牌失败：\(f)") }
        lines.append(push.usageNotes
            ? "额度通知：已配对电脑上的账号 5 小时或本周额度用完时推送一条，到重置时间再推送一条（电脑休眠时也会送达）。"
            : "额度用完或恢复时不推送。")
        if !push.closedToo, !push.unfiltered.isEmpty {
            lines.append("服务器「\(push.unfiltered.joined(separator: "」「"))」还不支持这个开关（需要管理员更新服务器）：经它配对的电脑，"
                + "已关闭的会话仍会推送。")
        }
        if push.usageNotes, !push.usageless.isEmpty {
            lines.append("服务器「\(push.usageless.joined(separator: "」「"))」还不支持额度通知（需要管理员更新服务器）。")
        }
        if !push.unsupported.isEmpty {
            lines.append("服务器「\(push.unsupported.joined(separator: "」「"))」还不支持推送（需要管理员更新服务器）：经它配对的电脑，"
                + "只有在 App 打开并接入会话时才能收到通知。")
        }
        return lines.joined(separator: "\n")
    }
}

/// 第三方的许可证原文（随 App 带着：Resources/Licenses）
private struct LicensesView: View {
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                Text("SwiftTerm").font(.headline)
                Text("终端显示使用 SwiftTerm（MIT 许可证）。").font(.callout).foregroundStyle(.secondary)
                Text(Self.text("SwiftTerm")).font(.caption.monospaced()).textSelection(.enabled)
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .navigationTitle("开源许可")
        .navigationBarTitleDisplayMode(.inline)
    }

    static func text(_ name: String) -> String {
        guard let url = Bundle.main.url(forResource: name, withExtension: "txt"),
              let s = try? String(contentsOf: url, encoding: .utf8) else { return "（未找到许可证原文）" }
        return s
    }
}
