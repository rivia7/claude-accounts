import AppKit
import SwiftUI

/// 菜单栏图标：模板图（跟着菜单栏变色）；路由器待同步、有槽位选错节点、或者有账号用量 ≥ 90% 时右上角带个点。
struct MenuBarLabel: View {
    let model: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        let attention = model.needsAttention
        Image(nsImage: MenuBarIcon.image(attention: attention))
            .accessibilityLabel(attention ? "cca（有需要注意的事）" : "cca")
            // 菜单栏图标启动时就有：在这里登记“打开主窗口”。--core 启动时主窗口和面板都还没建，Finder、cca gui 再打开 App
            // （applicationShouldHandleReopen → showMain）也能把主窗口开出来
            .onAppear { WindowOpener.shared.open = { openWindow(id: "main") } }
    }
}

enum MenuBarIcon {
    private static var cache: [Bool: NSImage] = [:]

    @MainActor
    static func image(attention: Bool) -> NSImage {
        if let img = cache[attention] { return img }
        let size = NSSize(width: 20, height: 16)
        let config = NSImage.SymbolConfiguration(pointSize: 13, weight: .medium)
        let symbol = NSImage(systemSymbolName: "point.3.connected.trianglepath.dotted", accessibilityDescription: "cca")?
            .withSymbolConfiguration(config)
        let img = NSImage(size: size, flipped: false) { rect in
            if let symbol {
                let s = symbol.size
                let origin = NSPoint(x: (rect.width - s.width) / 2 - (attention ? 1 : 0), y: (rect.height - s.height) / 2)
                symbol.draw(in: NSRect(origin: origin, size: s))
            }
            if attention {
                let dot = NSRect(x: rect.width - 7, y: rect.height - 7, width: 6, height: 6)
                NSGraphicsContext.current?.compositingOperation = .clear
                NSBezierPath(ovalIn: dot.insetBy(dx: -1.5, dy: -1.5)).fill()
                NSGraphicsContext.current?.compositingOperation = .sourceOver
                NSColor.black.setFill()
                NSBezierPath(ovalIn: dot).fill()
            }
            return true
        }
        img.isTemplate = true
        cache[attention] = img
        return img
    }
}

/// 菜单栏面板（约 340 pt 宽）：品牌标记和现在走哪边（待同步时右边是“同步”）、工具切换、
/// 每个账号一行（头像、名字、第二行、两行小用量、【打开】），底部是刷新用量、打开主窗口、退出。
struct MenuBarContent: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        @Bindable var model = model
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                BrandMark(size: 28)
                VStack(alignment: .leading, spacing: 1) {
                    Text("cca").font(.headline)
                    HStack(spacing: 5) {
                        if let issue = model.routeIssue {
                            Circle().fill(issue.1).frame(width: 6, height: 6)
                            Text(model.routeSummary + "，" + issue.0)
                        } else {
                            Text(model.routeSummary)
                        }
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                }
                Spacer(minLength: 6)
                if model.state?.routing?.sync == .pending, model.routerConfigured {
                    Button(model.syncActivity == nil ? "同步" : "同步中…") { model.syncRouter() }
                        .buttonStyle(.glass)
                        .controlSize(.small)
                        .disabled(model.syncActivity != nil)
                        .help("把这台 Mac 的账号表同步到路由器（可能要重载 OpenClash / ShellCrash，会先问你）")
                }
            }
            if model.visibleTools.count > 1 {
                Picker("工具", selection: $model.tool) {
                    ForEach(model.visibleTools, id: \.self) { Text(Texts.product($0)).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
            }
            content
            if let t = model.toast {
                Text(t.text).font(.caption).foregroundStyle(.secondary).lineLimit(2)
            }
            Divider()
            HStack(spacing: 14) {
                Button {
                    model.refreshUsage(manual: true)
                } label: {
                    Label(model.usageRefreshing.isEmpty ? "刷新用量" : "刷新中…", systemImage: "arrow.clockwise")
                }
                .disabled(!model.usageRefreshing.isEmpty || model.phase != .ready)
                Spacer()
                Button("打开主窗口") {
                    WindowOpener.shared.showMain()   // 只在菜单栏里托管内置代理时（--core 启动）顺便换回 Dock 图标
                }
                Button("退出") { NSApp.terminate(nil) }
            }
            .buttonStyle(.borderless)
            .font(.callout)
        }
        .padding(14)
        .frame(width: 340)
        .background(WindowObserver { visible in model.setPanelVisible(visible) })
        .onAppear {
            WindowOpener.shared.open = { openWindow(id: "main") }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch model.phase {
        case .ready:
            let accounts = model.accounts(model.tool)
            if accounts.isEmpty {
                Text(model.state == nil ? "正在读取…" : "还没有账号").foregroundStyle(.secondary).font(.callout)
            } else {
                VStack(spacing: 10) {
                    ForEach(accounts) { a in MenuRow(account: a) }
                }
            }
        case .starting:
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("正在连接 cca…").foregroundStyle(.secondary)
            }
        default:
            Text("cca 还没准备好：打开主窗口看看。").foregroundStyle(.secondary).font(.callout)
        }
    }
}

/// 面板里的一行：和侧栏同一套（头像、名字、第二行、两行小用量），右边是【打开】；有问题时名字后面一个橙色的点。
struct MenuRow: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        let a = account
        let u = model.usage(for: a)
        let busy = model.activity(a.ref) != nil
        let issue = problem(a, u)
        HStack(spacing: 10) {
            AccountAvatar(account: a, size: 28, dot: 9, visible: model.panelVisible)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Text(a.displayName).font(.callout.weight(.medium)).lineLimit(1)
                    if let issue {
                        Image(systemName: "exclamationmark.circle.fill")
                            .font(.system(size: 10))
                            .foregroundStyle(.orange)
                            .help(issue)
                            .accessibilityLabel(issue)
                    }
                }
                AccountSubtitle(account: a)
            }
            Spacer(minLength: 6)
            if let u, u.source != .none {
                UsageStack(usage: u, width: 26)
            }
            if busy {
                ProgressView().controlSize(.small).frame(width: 44)
            } else if !a.isAPI {
                // 第三方 API 账号没有桌面 App：不放按钮
                Button("打开") { model.openApp(a.ref) }
                    .buttonStyle(.glass)
                    .controlSize(.small)
                    .disabled(!a.can.app)
                    .accessibilityLabel("打开 \(a.displayName) 的 \(model.appLabel(a.tool))")
                    .help(a.can.app ? "打开 \(model.appLabel(a.tool))" : (a.canWhy["app"].map {
                        Texts.canWhy($0, appLabel: model.appLabel(a.tool), tool: a.tool)
                    } ?? "不能打开"))
            }
        }
    }

    /// 出错点：没登录、登错号、查用量出错。
    private func problem(_ a: Account, _ u: UsageEntry?) -> String? {
        switch a.login {
        case .loggedOut: return "还没登录"
        case .emailMismatch: return Texts.login(a.login, loginEmail: a.loginEmail)
        default: break
        }
        if let code = u?.errorCode { return nonEmpty(u?.error) ?? Texts.usageError(code) }
        return nil
    }
}
