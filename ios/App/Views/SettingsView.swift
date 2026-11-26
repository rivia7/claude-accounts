import CcaRemoteKit
import SwiftUI

/// 设置：外观、这台手机叫什么、配对过的电脑（取消配对）、通知（推送）、演示模式、关于
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
                    TextField("名字", text: $model.deviceName)
                        .textInputAutocapitalization(.never)
                        .onSubmit {
                            if Texts.nonEmpty(model.deviceName) == nil { model.deviceName = AppModel.defaultName }
                        }
                } header: {
                    Text("这台手机的名字")
                } footer: {
                    Text("配对时告诉电脑，电脑上的「配对过的机器」里显示它。改了只影响以后的配对。")
                }
                Section {
                    if model.peers.isEmpty {
                        Text("还没有配对过的电脑").foregroundStyle(.secondary)
                    }
                    ForEach(model.peers) { p in
                        HStack(spacing: 10) {
                            Image(systemName: "desktopcomputer").foregroundStyle(.secondary).frame(width: 22)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(p.name).lineLimit(1)
                                Text(Texts.peerDetail(p, now: Date().timeIntervalSince1970) + " · 经 " + p.server.host)
                                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
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
                    Text("配对过的电脑")
                } footer: {
                    if let p = model.peers.first, let fp = model.backend.fingerprint(on: p.server) {
                        Text("这台手机的指纹：\(fp.prefix(19))…（电脑上 cca remote peers 看得到同一个）")
                    }
                }
                Section {
                    LabeledContent("推送", value: pushState)
                    if !model.isDemo {
                        Toggle("关掉的会话也推送", isOn: Binding(get: { push.closedToo }, set: { push.closedToo = $0 }))
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
                        Button("看看演示") { model.enterDemo(); model.showSettings = false }
                    }
                } footer: {
                    Text("演示模式里的电脑和会话是模拟的，不联网，也不动这台手机上真的配对。")
                }
                Section("关于") {
                    LabeledContent("版本", value: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?")
                    NavigationLink("开源许可") { LicensesView() }
                }
            }
            .navigationTitle("设置")
            .navigationBarTitleDisplayMode(.inline)
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
                Text("这台手机就不能再接入那台电脑上的会话了，正接着的会断开。以后要用得重新配对。")
            }
        }
    }
}

extension SettingsView {
    /// 推送现在怎么样（一个词）
    private var pushState: String {
        if model.isDemo { return "演示模式里没有" }
        switch push.allowed {
        case false: return "关着"
        case nil: return "还没问过"
        default: return push.token == nil ? (push.failure == nil ? "正在打开" : "没打开") : "开着"
        }
    }

    private var pushFooter: String {
        var lines = ["锁着屏、App 没开着时也收得到会话里的通知（做完了、在等你）：经那台电脑所在的服务器和 cca 的推送服务转过来，"
            + "内容封着，只有这台手机解得开。"]
        if model.isDemo { return lines[0] }
        lines.append(push.closedToo
            ? "关掉的会话也推送：配对的电脑上的会话都推，在 App 里关掉了的也推。关掉这一项就只推 App 里开着的那个会话。"
            : "只推 App 里开着的那个会话（锁着屏、切到别的 App 也算开着），关掉以后就不推了。")
        if push.allowed == false { lines.append("通知关着：在系统的「设置」→「通知」→「cca」里打开。") }
        if let f = push.failure, push.token == nil { lines.append("向 Apple 要推送的令牌没成：\(f)") }
        lines.append(push.usageNotes
            ? "额度用完、恢复的通知：配对的电脑上有账号的 5 小时或本周额度用完了推一条，到了重置的时间再推一条（那台电脑睡着了也到）。"
            : "额度用完了、恢复了不推。")
        if !push.closedToo, !push.unfiltered.isEmpty {
            lines.append("「\(push.unfiltered.joined(separator: "」「"))」这台服务器还不认这个开关（要管理员更新服务器）：经它配对的电脑，"
                + "关掉的会话照样推。")
        }
        if push.usageNotes, !push.usageless.isEmpty {
            lines.append("「\(push.usageless.joined(separator: "」「"))」这台服务器还推不了额度的通知（要管理员更新服务器）。")
        }
        if !push.unsupported.isEmpty {
            lines.append("「\(push.unsupported.joined(separator: "」「"))」这台服务器还没接上推送（要管理员更新服务器）：经它配对的电脑，"
                + "只在 App 开着、接着会话时收得到通知。")
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
                Text("终端的显示用的是 SwiftTerm（MIT 许可证）。").font(.callout).foregroundStyle(.secondary)
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
              let s = try? String(contentsOf: url, encoding: .utf8) else { return "（没找到许可证的原文）" }
        return s
    }
}
