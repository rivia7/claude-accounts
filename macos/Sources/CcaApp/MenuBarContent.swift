import AppKit
import SwiftUI

/// 菜单栏图标：模板图（跟着菜单栏变色）；路由器待同步、有槽位选错节点、或者有账号用量 ≥ 90% 时右上角带个点。
struct MenuBarLabel: View {
    let model: AppModel

    var body: some View {
        let attention = model.needsAttention
        Image(nsImage: MenuBarIcon.image(attention: attention))
            .accessibilityLabel(attention ? "cca（有需要注意的事）" : "cca")
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

/// 菜单栏面板（参考 magpie 的面板，约 360 pt 宽）：左上角品牌，右边刷新用量、打开主窗口；
/// 每个工具一张白色的列表（一行一个账号：头像、名字、5h / 周的用量、【打开】），分流有问题时一行提示，底部打开 cca、退出。
struct MenuBarContent: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 6) {
                BrandLabel()
                Spacer()
                Button {
                    model.refreshUsage(manual: true)
                } label: {
                    if model.usageRefreshing.isEmpty {
                        Image(systemName: "arrow.clockwise")
                    } else {
                        ProgressView().controlSize(.mini)
                    }
                }
                .buttonStyle(.icon)
                .disabled(!model.usageRefreshing.isEmpty || model.phase != .ready)
                .help("经各账号的出口查一次用量")
                .accessibilityLabel("刷新用量")
                Button {
                    showMain()
                } label: {
                    Image(systemName: "macwindow")
                }
                .buttonStyle(.icon)
                .help("打开 cca 的主窗口")
                .accessibilityLabel("打开主窗口")
            }
            content
            routeLine
            if let t = model.toast {
                Text(t.text).font(.system(size: 11.5)).foregroundStyle(Theme.muted).lineLimit(2)
            }
            HStack(spacing: 2) {
                Spacer()
                Button("打开 cca") { showMain() }.buttonStyle(.quiet)
                Button("退出") { NSApp.terminate(nil) }.buttonStyle(.quiet)
            }
        }
        .padding(12)
        .frame(width: 360)
        .background(Theme.bg)
        .background(WindowObserver { visible in model.setPanelVisible(visible) })
        .onAppear {
            WindowOpener.shared.open = { openWindow(id: "main") }
        }
    }

    private func showMain() {
        openWindow(id: "main")
        NSApp.activate()
    }

    @ViewBuilder
    private var content: some View {
        switch model.phase {
        case .ready:
            let tools = model.visibleTools.filter { !model.accounts($0).isEmpty }
            if tools.isEmpty {
                Text(model.state == nil ? "正在读取…" : "还没有账号").font(.system(size: 12.5)).foregroundStyle(Theme.muted)
            } else {
                ForEach(tools, id: \.self) { t in
                    VStack(alignment: .leading, spacing: 4) {
                        if tools.count > 1 {
                            Text(Texts.product(t)).font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Theme.muted)
                                .padding(.horizontal, 4)
                        }
                        VStack(spacing: 0) {
                            ForEach(Array(model.accounts(t).enumerated()), id: \.element.id) { i, a in
                                if i > 0 { Hairline() }
                                MenuRow(account: a)
                            }
                        }
                        .card()
                    }
                }
            }
        case .starting:
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("正在连接 cca…").font(.system(size: 12.5)).foregroundStyle(Theme.muted)
            }
        default:
            Text("cca 还没准备好：打开主窗口看看。").font(.system(size: 12.5)).foregroundStyle(Theme.muted)
        }
    }

    /// 分流有问题时一行：路由器待同步（右边“同步”）、有槽位连不上 / 选错节点。
    @ViewBuilder
    private var routeLine: some View {
        if let issue = model.routeIssue {
            HStack(spacing: 7) {
                StatusDot(tone: issue.1, size: 7)
                Text(model.routeSummary + "：" + issue.0).font(.system(size: 12)).foregroundStyle(Theme.fg2).lineLimit(1)
                Spacer(minLength: 6)
                if model.state?.routing?.sync == .pending, model.routerConfigured {
                    Button(model.syncActivity == nil ? "同步" : "同步中…") { model.syncRouter() }
                        .buttonStyle(.action)
                        .disabled(model.syncActivity != nil)
                        .help("把这台 Mac 的账号表同步到路由器（可能要重载 OpenClash / ShellCrash，会先问你）")
                }
            }
            .padding(.horizontal, 4)
        }
    }
}

/// 面板里的一行：头像、名字（有问题时后面一个点）、5h / 周的用量，右边【打开】（第三方 API 账号没有）。
struct MenuRow: View {
    @Environment(AppModel.self) private var model
    let account: Account

    var body: some View {
        let a = account
        let u = model.usage(for: a)
        let busy = model.activity(a.ref) != nil
        let issue = problem(a, u)
        HStack(spacing: 10) {
            BadgeAvatar(badge: a.badge, isAPI: a.isAPI, size: 24)
                .overlay(alignment: .bottomTrailing) {
                    if a.appRunning {
                        RunningDot(size: 8).offset(x: 2, y: 2)
                            .accessibilityLabel("\(model.appLabel(a.tool)) 开着")
                    }
                }
            HStack(spacing: 5) {
                Text(a.shortName).font(.system(size: 13, weight: .medium)).foregroundStyle(Theme.fg).lineLimit(1)
                if let issue {
                    StatusDot(tone: .warn, size: 6)
                        .help(issue)
                        .accessibilityLabel(issue)
                }
            }
            Spacer(minLength: 6)
            if let u, u.source != .none {
                RowQuota(usage: u, width: 50)
            } else if a.login != .loggedIn, !a.isAPI {
                Text(Texts.login(a.login, loginEmail: nil)).font(.system(size: 11.5)).foregroundStyle(Theme.muted)
            }
            Group {
                if busy {
                    ProgressView().controlSize(.small)
                } else if a.login == .loggedOut || a.login == .emailMismatch {
                    Button("登录") { model.runInTerminal(model.loginHandoff(a)) }
                        .buttonStyle(.action)
                        .help("在终端里运行 \(model.loginHandoff(a).display)")
                } else if !a.isAPI {
                    Button("打开") { model.openApp(a.ref) }
                        .buttonStyle(.action)
                        .disabled(!a.can.app)
                        .accessibilityLabel("打开 \(a.displayName) 的 \(model.appLabel(a.tool))")
                        .help(a.can.app ? "打开 \(model.appLabel(a.tool))" : (a.canWhy["app"].map {
                            Texts.canWhy($0, appLabel: model.appLabel(a.tool), tool: a.tool)
                        } ?? "不能打开"))
                }
            }
            .frame(width: 48, alignment: .trailing)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .help(a.displayName)
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
