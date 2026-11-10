#if DEBUG
import AppKit
import ScreenCaptureKit
import SwiftUI

/// 只在调试版里（build.sh --debug）：演示模式下设了 CCA_GUI_SNAPSHOT=<目录> 时，依次切到几个场景，
/// 把主窗口和菜单栏面板截成 PNG（浅色、深色各一张）后退出。看界面改得怎么样用，不进发布版。
///
///     CCA_GUI_FIXTURES=macos/Tests/Fixtures CCA_GUI_SNAPSHOT=/tmp/shots macos/build/cca.app/Contents/MacOS/cca
///
/// 只截自己的窗口（SCShareableContent.currentProcess），不用录屏权限；窗口放到别的窗口后面，不抢焦点。
/// CCA_GUI_SNAPSHOT_SCENES=claude,routing 只截这几个场景。
@MainActor
enum Snapshot {
    static var directory: URL? {
        let env = ProcessInfo.processInfo.environment
        guard let dir = env["CCA_GUI_SNAPSHOT"], !dir.isEmpty, !(env["CCA_GUI_FIXTURES"] ?? "").isEmpty else { return nil }
        return URL(fileURLWithPath: dir)
    }

    private struct Scene {
        let name: String
        let apply: @MainActor (AppModel) -> Void
    }

    static func run(model: AppModel, guide: Guide, into dir: URL) async {
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        NSApp.setActivationPolicy(.accessory)
        // 等连上（演示数据）、读到 state
        for _ in 0..<100 where !(model.phase == .ready && model.state != nil) {
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        guard let window = await mainWindow() else {
            FileHandle.standardError.write(Data("snapshot: 没找到主窗口\n".utf8))
            exit(1)
        }
        let size = sizeFromEnvironment() ?? NSSize(width: 1080, height: 720)
        window.setContentSize(size)
        window.orderBack(nil)
        model.refreshUsage(manual: false)
        model.refreshLive()
        // 第一次打开时的卡片教程先关掉（它自己有场景 tutorial）
        guide.finishTutorial()
        try? await Task.sleep(nanoseconds: 2_500_000_000)

        func first(_ t: ToolID, _ match: (Account) -> Bool) -> AccountRef? {
            model.accounts(t).first(where: match)?.ref
        }
        let scenes: [Scene] = [
            Scene(name: "claude") { m in
                m.tool = .claude
                m.selection = first(.claude) { $0.login == .loggedIn && !$0.isAPI }.map { .account($0) }
                if let r = m.selection?.account { m.refreshDevices(r, manual: true) }   // 窗口在后面，不算看得见：「设备」自己查一次
            },
            Scene(name: "claude-loggedout") { m in
                m.tool = .claude
                m.selection = first(.claude) { $0.login != .loggedIn && !$0.isAPI }.map { .account($0) }
            },
            Scene(name: "claude-api") { m in
                m.tool = .claude
                m.selection = first(.claude) { $0.isAPI && $0.health != nil }.map { .account($0) }
            },
            Scene(name: "routing") { m in
                m.selection = .routing
            },
            Scene(name: "codex") { m in
                guard m.visibleTools.contains(.codex) else { return }
                m.tool = .codex
                m.selection = first(.codex) { _ in true }.map { .account($0) }
            },
            Scene(name: "empty") { m in
                m.tool = .claude
                m.selection = nil
            },
            Scene(name: "office") { m in
                m.selection = .office
            },
            Scene(name: "client-updates") { m in
                m.selection = .clientUpdates
            },
            Scene(name: "stats") { m in
                m.selection = .usageStats
            },
            // 置顶（演示模式不存进设置）：放在最后，别的场景不受影响
            Scene(name: "pinned") { m in
                m.tool = .claude
                if let a = m.accounts(.claude).last(where: { !$0.isAPI && $0.login == .loggedIn }), !m.isOnTop(a) { m.setOnTop(a, true) }
                m.selection = .office
            },
        ]
        let only = Set((ProcessInfo.processInfo.environment["CCA_GUI_SNAPSHOT_SCENES"] ?? "")
            .split(separator: ",").map(String.init))
        let appearances: [(String, NSAppearance.Name)] = [("light", .aqua), ("dark", .darkAqua)]
        for scene in scenes where only.isEmpty || only.contains(scene.name) {
            scene.apply(model)
            if scene.name == "routing" || scene.name == "office" { model.refreshLive() }
            for (label, name) in appearances {
                NSApp.appearance = NSAppearance(named: name)
                try? await Task.sleep(nanoseconds: 900_000_000)
                await capture(window, to: dir.appendingPathComponent("\(scene.name)-\(label).png"))
            }
        }
        // 显示细节时的账号页、分流页
        if only.isEmpty || only.contains("details") {
            guide.toggleDetails()
            for (scene, apply) in [("claude-details", scenes[0].apply), ("routing-details", scenes[3].apply)] {
                apply(model)
                NSApp.appearance = NSAppearance(named: .aqua)
                try? await Task.sleep(nanoseconds: 900_000_000)
                await capture(window, to: dir.appendingPathComponent("\(scene)-light.png"))
            }
            guide.toggleDetails()
        }
        // 直接改别名：名字变成输入框
        if only.isEmpty || only.contains("rename"), let ref = first(.claude, { $0.login == .loggedIn && !$0.isAPI }) {
            model.beginRename(ref)
            for (label, name) in appearances {
                NSApp.appearance = NSAppearance(named: name)
                try? await Task.sleep(nanoseconds: 1_000_000_000)
                await capture(window, to: dir.appendingPathComponent("rename-\(label).png"))
            }
            model.cancelRename()
            try? await Task.sleep(nanoseconds: 600_000_000)
        }
        // 卡片教程：每一步截一张弹层（浅色、深色）
        if only.isEmpty || only.contains("tutorial") {
            for step in 0..<TutorialCard.all.count {
                guide.openTutorial()
                guide.step = step
                for (label, name) in appearances {
                    NSApp.appearance = NSAppearance(named: name)
                    try? await Task.sleep(nanoseconds: 1_000_000_000)
                    if let sheet = window.attachedSheet {
                        await capture(sheet, to: dir.appendingPathComponent("tutorial-\(step + 1)-\(label).png"))
                    }
                }
            }
            guide.sheet = nil
            try? await Task.sleep(nanoseconds: 600_000_000)
        }
        // 在 App 里问完的流程（提问会话）：每一页截一张弹层（CCA_GUI_FIXTURES 里的 flow_*.flow）
        if only.isEmpty || only.contains("flow") {
            func demo(_ name: String, _ title: String) -> Handoff { Handoff(arguments: [name], title: title, inApp: true) }
            let add = Handoff.addAccount(prefix: [], product: "Claude")
            let flows: [(String, Handoff, Int)] = [
                ("flow-add", add, 0), ("flow-add-error", add, 1), ("flow-add-node", add, 2),
                ("flow-api", .addAPI(), 0), ("flow-api-node", .addAPI(), 1),
                ("flow-key", .apiKey(prefix: [], key: "kimi", name: "kimi"), 0),
                ("flow-router", .routerSetup, 0), ("flow-router-password", .routerSetup, 1), ("flow-router-nodes", .routerSetup, 2),
                ("flow-local", .localSetup, 0), ("flow-local-manual", .localSetup, 1),
                ("flow-edit", .edit(prefix: []), 0),
                ("flow-rm", .remove(prefix: [], key: "bob", name: "工作"), 0),
                ("flow-working", demo("demo-working", "添加 Claude 账号"), 0),
                ("flow-warned", demo("demo-warned", "添加 Claude 账号"), 0),
                ("flow-failed", demo("demo-failed", "添加第三方 API 账号"), 0),
            ]
            for (name, handoff, pages) in flows {
                model.run(handoff)
                try? await Task.sleep(nanoseconds: 500_000_000)
                for _ in 0..<pages {
                    guard let f = model.flow, case .asking(let e) = f.phase else { break }
                    for x in e.fields {
                        if x.kind == .secret { f.setSecret(x.id, "demo-demo") } else if (f.draft[x.id] ?? "").isEmpty { f.draft[x.id] = "demo" }
                    }
                    f.submit()
                    try? await Task.sleep(nanoseconds: 400_000_000)
                }
                for (label, appearance) in appearances {
                    NSApp.appearance = NSAppearance(named: appearance)
                    try? await Task.sleep(nanoseconds: 1_000_000_000)
                    if let sheet = window.attachedSheet {
                        await capture(sheet, to: dir.appendingPathComponent("\(name)-\(label).png"))
                    }
                }
                model.flow?.cancel()
                try? await Task.sleep(nanoseconds: 700_000_000)
            }
        }
        // 更新了什么：每个版本一张（WhatsNew.all：开发版里还没发布的那一版也截）
        if only.isEmpty || only.contains("whatsnew") {
            guide.showWhatsNew(WhatsNew.all)
            for i in WhatsNew.all.indices {
                guide.step = i
                for (label, name) in appearances {
                    NSApp.appearance = NSAppearance(named: name)
                    try? await Task.sleep(nanoseconds: 1_000_000_000)
                    if let sheet = window.attachedSheet {
                        await capture(sheet, to: dir.appendingPathComponent("whatsnew-\(i + 1)-\(label).png"))
                    }
                }
            }
            guide.sheet = nil
            try? await Task.sleep(nanoseconds: 600_000_000)
        }
        // 检查更新找到新版本：窗口顶上的提示条（演示数据的 update.json；放在后面，前面的场景不带它）
        if only.isEmpty || only.contains("update") {
            model.selection = .office
            model.checkForUpdates(manual: false)
            for (label, name) in appearances {
                NSApp.appearance = NSAppearance(named: name)
                try? await Task.sleep(nanoseconds: 1_000_000_000)
                await capture(window, to: dir.appendingPathComponent("update-\(label).png"))
            }
        }
        // 工位画廊：每种状态 × 几种档位（调插画用，不依赖演示数据）
        if only.isEmpty || only.contains("gallery") {
            for (label, name) in appearances {
                NSApp.appearance = NSAppearance(named: name)
                let panel = galleryPanel(model: model, guide: guide)
                try? await Task.sleep(nanoseconds: 1_200_000_000)
                await capture(panel, to: dir.appendingPathComponent("gallery-\(label).png"))
                panel.close()
            }
        }
        if only.isEmpty || only.contains("menubar") {
            model.tool = .claude
            for (label, name) in appearances {
                NSApp.appearance = NSAppearance(named: name)
                let panel = menuPanel(model: model)
                try? await Task.sleep(nanoseconds: 900_000_000)
                await capture(panel, to: dir.appendingPathComponent("menubar-\(label).png"))
                panel.close()
            }
        }
        exit(0)
    }

    private static func sizeFromEnvironment() -> NSSize? {
        let parts = (ProcessInfo.processInfo.environment["CCA_GUI_SNAPSHOT_SIZE"] ?? "").split(separator: "x").compactMap { Double($0) }
        return parts.count == 2 ? NSSize(width: parts[0], height: parts[1]) : nil
    }

    private static func mainWindow() async -> NSWindow? {
        for _ in 0..<50 {
            if let w = NSApp.windows.first(where: { $0.isVisible && $0.canBecomeMain && !($0 is NSPanel) }) { return w }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        return nil
    }

    /// 菜单栏面板的近似：圆角的毛玻璃面板里放 MenuBarContent（真正的面板由系统在点图标时弹出，没法直接截）。
    private static func menuPanel(model: AppModel) -> NSPanel {
        let host = NSHostingView(rootView: MenuBarContent().environment(model))
        let fit = host.fittingSize
        let panel = NSPanel(contentRect: NSRect(x: 260, y: 240, width: fit.width, height: fit.height),
                            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        let glass = NSGlassEffectView()
        glass.cornerRadius = 18
        glass.contentView = host
        panel.contentView = glass
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.orderBack(nil)
        return panel
    }

    private static func galleryPanel(model: AppModel, guide: Guide) -> NSPanel {
        let host = NSHostingView(rootView: DeskGallery().environment(model).environment(guide))
        let fit = host.fittingSize
        let panel = NSPanel(contentRect: NSRect(x: 40, y: 40, width: fit.width, height: fit.height),
                            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.contentView = host
        panel.backgroundColor = .windowBackgroundColor
        panel.orderBack(nil)
        return panel
    }

    private static func capture(_ window: NSWindow, to url: URL) async {
        do {
            let content = try await SCShareableContent.currentProcess
            guard let sw = content.windows.first(where: { $0.windowID == CGWindowID(window.windowNumber) }) else {
                FileHandle.standardError.write(Data("snapshot: 截不到窗口 \(window.windowNumber)\n".utf8))
                return
            }
            let filter = SCContentFilter(desktopIndependentWindow: sw)
            let cfg = SCStreamConfiguration()
            cfg.width = Int(filter.contentRect.width * CGFloat(filter.pointPixelScale))
            cfg.height = Int(filter.contentRect.height * CGFloat(filter.pointPixelScale))
            cfg.showsCursor = false
            cfg.ignoreShadowsSingleWindow = true
            let img = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: cfg)
            let rep = NSBitmapImageRep(cgImage: img)
            try rep.representation(using: .png, properties: [:])?.write(to: url)
            print(url.path)
        } catch {
            FileHandle.standardError.write(Data("snapshot: \(url.lastPathComponent) 截图失败：\(error)\n".utf8))
        }
    }
}
/// 工位画廊：一行一种账号（几种账号色、第三方 API 的笔记本；套餐档位不进插画），一列一种状态。
private struct DeskGallery: View {
    private struct Look { let caption: String; let mood: Employee.Mood; let app: Bool; let sessions: Int }
    private struct Tier { let caption: String; let plan: String; let tier: PlanTier; let api: Bool; let color: String }

    private static let looks = [
        Look(caption: "App + 命令行", mood: .working, app: true, sessions: 2), Look(caption: "命令行", mood: .working, app: false, sessions: 1),
        Look(caption: "App", mood: .working, app: true, sessions: 0), Look(caption: "空闲", mood: .idle, app: false, sessions: 0),
        Look(caption: "额度用完", mood: .resting(until: Date().timeIntervalSince1970 + 7800), app: false, sessions: 0),
        Look(caption: "没登录", mood: .absent, app: false, sessions: 0),
        Look(caption: "登错号", mood: .wrongPerson(loginEmail: "eve@example.com"), app: false, sessions: 0),
        Look(caption: "端口不通", mood: .unwell(.portClosed), app: true, sessions: 0),
        Look(caption: "选错节点", mood: .unwell(.wrongNode), app: false, sessions: 0),
    ]
    private static let tiers = [
        Tier(caption: "绿色", plan: "Max", tier: .advanced, api: false, color: "#30A46C"),
        Tier(caption: "紫色", plan: "Max", tier: .advanced, api: false, color: "#8E4EC6"),
        Tier(caption: "蓝色", plan: "Pro", tier: .standard, api: false, color: "#0A84FF"),
        Tier(caption: "第三方 API", plan: "", tier: .unknown, api: true, color: ""),
    ]

    var body: some View {
        let now = Date().timeIntervalSince1970
        Grid(horizontalSpacing: 10, verticalSpacing: 10) {
            ForEach(Self.tiers, id: \.caption) { t in
                GridRow {
                    Text(t.caption).font(.caption.weight(.semibold))
                    ForEach(Self.looks, id: \.caption) { l in
                        VStack(spacing: 4) {
                            DeskScene(employee: Self.employee(t, l, now: now), animate: false, now: now).frame(width: 168)
                            Text(l.caption).font(.caption2).foregroundStyle(.secondary)
                        }
                        .padding(6)
                        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Surface.tile))
                    }
                }
            }
        }
        .padding(16)
    }

    private static func employee(_ t: Tier, _ l: Look, now: Double) -> Employee {
        var d: [String: Any] = ["key": t.api ? "接口" : "demo@example.com", "kind": t.api ? "api" : "oauth",
                                "login": t.api ? "api" : "logged_in", "plan": t.plan, "plan_tier": t.tier.rawValue,
                                "app": ["running": l.app], "cli": ["running": l.sessions > 0, "sessions": l.sessions, "since_epoch": now - 600]]
        if !t.api { d["badge"] = ["label": "工", "color": t.color] }
        return Employee(account: Account.sample(d), mood: l.mood, flags: [])
    }
}
#endif
