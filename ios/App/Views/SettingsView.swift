import CcaRemoteKit
import SwiftUI

/// 设置：外观、这台手机叫什么、配对过的电脑（取消配对）、演示模式、关于
struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @State private var unpairing: Peer?

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
