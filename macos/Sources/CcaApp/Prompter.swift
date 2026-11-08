import AppKit
import SwiftUI

/// 要用户同意的事（PROTOCOL 4.2）：先把 App 切到前面，再弹原生对话框。对话框只用事件里的
/// title / question / detail / severity / default：warn → 警告样式；default 决定哪个是默认按钮（回车）。
final class AlertPrompter: Prompter, @unchecked Sendable {
    func confirm(_ consent: ConsentEvent) async -> Bool {
        await MainActor.run {
            NSApp.activate()
            let alert = NSAlert()
            alert.messageText = consent.title.isEmpty ? "需要你同意" : consent.title
            var info = consent.question
            if !consent.detail.isEmpty { info += (info.isEmpty ? "" : "\n\n") + consent.detail }
            alert.informativeText = info
            alert.alertStyle = consent.severity == .warn ? .warning : .informational
            if consent.defaultYes {
                alert.addButton(withTitle: "是")
                let no = alert.addButton(withTitle: "否")
                no.keyEquivalent = "\u{1b}"
                return alert.runModal() == .alertFirstButtonReturn
            } else {
                alert.addButton(withTitle: "否")
                alert.addButton(withTitle: "是")
                return alert.runModal() == .alertSecondButtonReturn
            }
        }
    }
}

/// 主窗口上的弹层：终端交接、失败说明。
struct SheetView: View {
    let sheet: Sheet

    var body: some View {
        switch sheet {
        case .handoff(let h): HandoffSheet(handoff: h)
        case .failure(let f): FailureSheet(info: f)
        }
    }
}

/// “需要在终端里完成”：【在终端里运行】【复制命令】【取消】（PROTOCOL 4.3 第 4 条）。
struct HandoffSheet: View {
    @Environment(AppModel.self) private var model
    let handoff: Handoff

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 12) {
                Image(systemName: "terminal.fill")
                    .font(.system(size: 26))
                    .foregroundStyle(.tint)
                VStack(alignment: .leading, spacing: 2) {
                    Text(handoff.title).font(.title3.weight(.semibold))
                    Text("这一步要在“终端”里完成，完成后回到 cca 会自动刷新。").font(.callout).foregroundStyle(.secondary)
                }
            }
            if !handoff.message.isEmpty {
                Text(handoff.message).fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
            }
            Text(handoff.display)
                .font(.system(.body, design: .monospaced))
                .textSelection(.enabled)
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 8).fill(.quaternary.opacity(0.6)))
            HStack {
                Spacer()
                Button("取消") { model.dismissSheet() }
                    .keyboardShortcut(.cancelAction)
                Button("复制命令") { model.copy(handoff.display) }
                Button("在终端里运行") {
                    model.runInTerminal(handoff)
                    model.dismissSheet()
                }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.glassProminent)
            }
        }
        .padding(22)
        .frame(width: 480)
    }
}

/// 失败：最后一条 fail 的 message，按 code 给补救按钮，stderr 的最后几行放在“详情”里（PROTOCOL 4.3 第 5 条）。
struct FailureSheet: View {
    @Environment(AppModel.self) private var model
    let info: FailureInfo

    var body: some View {
        let f = info.failure
        let remedy = model.remedy(for: f)
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 12) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.system(size: 26))
                    .foregroundStyle(.orange)
                Text(info.title).font(.title3.weight(.semibold))
            }
            Text(f.message)
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
            if !f.stderrTail.isEmpty {
                DisclosureGroup("详情") {
                    ScrollView {
                        Text(f.stderrTail)
                            .font(.system(.caption, design: .monospaced))
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(8)
                    }
                    .frame(maxHeight: 180)
                    .background(RoundedRectangle(cornerRadius: 8).fill(.quaternary.opacity(0.6)))
                }
            }
            HStack {
                Spacer()
                if let title = remedy.title {
                    Button(title) {
                        model.dismissSheet()
                        model.apply(remedy)
                    }
                    .buttonStyle(.glass)
                }
                Button("好") { model.dismissSheet() }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.glassProminent)
            }
        }
        .padding(22)
        .frame(width: 500)
    }
}

enum Platform {
    /// 这台 Mac 上装了哪些认识的终端（系统自带的一定有）。
    @MainActor
    static func installedTerminals() -> [TerminalApp] {
        TerminalApp.known.filter { $0 == .system || NSWorkspace.shared.urlForApplication(withBundleIdentifier: $0.id) != nil }
    }

    /// 用选好的终端打开交接文件（都不需要“自动化”权限）；打不开时回调 false。
    @MainActor
    static func open(_ script: URL, in terminal: TerminalApp, done: @escaping @MainActor (Bool) -> Void) {
        guard let app = NSWorkspace.shared.urlForApplication(withBundleIdentifier: terminal.id) else {
            done(false)
            return
        }
        let finish: (Error?) -> Void = { error in
            DispatchQueue.main.async { MainActor.assumeIsolated { done(error == nil) } }
        }
        let cfg = NSWorkspace.OpenConfiguration()
        cfg.activates = true
        switch terminal.launch {
        case .openFile:
            NSWorkspace.shared.open([script], withApplicationAt: app, configuration: cfg) { _, error in finish(error) }
        case .arguments(let arguments):
            // 相当于 open -na 终端 --args …：它已经开着时，普通的打开不会把参数带过去
            cfg.createsNewApplicationInstance = true
            cfg.arguments = arguments.map { $0 == TerminalApp.scriptToken ? script.path : $0 }
            NSWorkspace.shared.openApplication(at: app, configuration: cfg) { _, error in finish(error) }
        }
    }

    /// 选一个文件夹；取消时是 nil。
    @MainActor
    static func chooseDirectory(startingAt path: String?) async -> String? {
        NSApp.activate()
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = true
        panel.prompt = "在这里打开"
        panel.message = "选择在哪个文件夹里打开（下次默认还是这里）"
        if let path { panel.directoryURL = URL(fileURLWithPath: path, isDirectory: true) }
        return await panel.begin() == .OK ? panel.url?.path : nil
    }

    @MainActor
    static func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}
