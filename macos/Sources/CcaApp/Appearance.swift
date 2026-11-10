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
                LabeledContent("默认终端") {
                    Menu(TerminalMenuItems.label(model.terminalChoice, installed: true)) { TerminalMenuItems() }
                        .fixedSize()
                }
                Text(terminalCaption)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Section("新手引导") {
                Toggle("显示细节（端口、纯净度这些明细）",
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

    private var terminalCaption: String {
        let t = model.terminalChoice
        guard t.isAutomatic else {
            return "别的 App 没法让 \(t.name) 运行命令：点账号页的「终端」会打开它、把命令复制好，到它里面粘贴运行。登录、添加账号总是用系统自带的终端。"
        }
        return (["账号页的「终端」用它打开；登录、添加账号这些命令总是用系统自带的终端。"] + [t.note].compactMap { $0 }).joined(separator: "")
    }
}

/// 选默认终端的菜单项：认识的都列出来，没装的灰着不能选，半自动的写明。
/// 用 Toggle 不用 Picker：Picker 的选项不认 .disabled（实测菜单里全是可选的）。
struct TerminalMenuItems: View {
    @Environment(AppModel.self) private var model

    static func label(_ t: TerminalApp, installed: Bool) -> String {
        guard installed else { return "\(t.name)（未安装）" }
        return t.isAutomatic ? t.name : "\(t.name)（半自动）"
    }

    var body: some View {
        ForEach(TerminalApp.known) { t in
            let installed = model.isInstalled(t)
            Toggle(Self.label(t, installed: installed), isOn: Binding(get: { model.terminalChoice == t }, set: { if $0 { model.chooseTerminal(t.id) } }))
                .disabled(!installed)
        }
    }
}
