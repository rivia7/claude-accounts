import AppKit
import SwiftUI

/// 外观：跟随系统、浅色、深色。
enum AppearanceMode: String, CaseIterable, Identifiable {
    case system, light, dark

    var id: String { rawValue }

    var title: String {
        switch self {
        case .system: return "跟随系统"
        case .light: return "浅色"
        case .dark: return "深色"
        }
    }

    var symbol: String {
        switch self {
        case .system: return "circle.lefthalf.filled"
        case .light: return "sun.max"
        case .dark: return "moon"
        }
    }
}

/// 外观的设置：记在 App 的设置里（演示模式只记在内存里），用 NSApp.appearance 套到所有窗口（主窗口、菜单栏面板、弹框）。
@MainActor
@Observable
final class AppearanceSetting {
    @ObservationIgnored private let prefs: PrefsStore
    private(set) var mode: AppearanceMode

    init(prefs: PrefsStore) {
        self.prefs = prefs
        mode = AppearanceMode(rawValue: prefs.string(forKey: "appearance") ?? "") ?? .system
    }

    func set(_ m: AppearanceMode) {
        mode = m
        prefs.set(m.rawValue, forKey: "appearance")
        apply()
    }

    func apply() {
        switch mode {
        case .system: NSApp.appearance = nil
        case .light: NSApp.appearance = NSAppearance(named: .aqua)
        case .dark: NSApp.appearance = NSAppearance(named: .darkAqua)
        }
    }

    var binding: Binding<AppearanceMode> {
        Binding(get: { self.mode }, set: { self.set($0) })
    }
}

/// 侧栏底部的外观按钮：图标是现在的外观，点开选跟随系统 / 浅色 / 深色。
struct AppearanceMenu: View {
    @Environment(AppearanceSetting.self) private var appearance

    var body: some View {
        Menu {
            Picker("外观", selection: appearance.binding) {
                ForEach(AppearanceMode.allCases) { m in
                    Label(m.title, systemImage: m.symbol).tag(m)
                }
            }
            .pickerStyle(.inline)
            .labelsHidden()
        } label: {
            Image(systemName: appearance.mode.symbol)
        }
        .menuStyle(.button)
        .buttonStyle(.borderless)
        .menuIndicator(.hidden)
        .fixedSize()
        .help("外观：\(appearance.mode.title)（浅色 / 深色 / 跟随系统）")
        .accessibilityLabel("外观：\(appearance.mode.title)")
    }
}

/// 设置窗口（⌘,）：外观、新手引导。
struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(AppearanceSetting.self) private var appearance
    @Environment(Guide.self) private var guide

    var body: some View {
        Form {
            Section("外观") {
                Picker("外观", selection: appearance.binding) {
                    ForEach(AppearanceMode.allCases) { m in
                        Label(m.title, systemImage: m.symbol).tag(m)
                    }
                }
                .pickerStyle(.segmented)
            }
            Section("终端") {
                Picker("默认终端", selection: Binding(get: { model.terminalChoice.id }, set: { model.chooseTerminal($0) })) {
                    ForEach(model.terminals) { t in
                        Text(t.name).tag(t.id)
                    }
                }
                Text("账号页的「终端」、登录、添加账号都用它打开。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Section("新手引导") {
                Toggle("显示技术细节（端口、槽位、纯净度明细）",
                       isOn: Binding(get: { guide.showDetails }, set: { if $0 != guide.showDetails { guide.toggleDetails() } }))
                HStack {
                    Button("看新手教程") {
                        WindowOpener.shared.showMain()
                        guide.openTutorial()
                    }
                    Button("重新显示提示卡片") { guide.resetTips() }
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 460)
        .onAppear { model.refreshTerminals() }
        .fixedSize(horizontal: false, vertical: true)
    }
}
