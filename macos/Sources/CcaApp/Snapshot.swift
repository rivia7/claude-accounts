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

    static func run(model: AppModel, into dir: URL) async {
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
        try? await Task.sleep(nanoseconds: 2_500_000_000)

        func first(_ t: ToolID, _ match: (Account) -> Bool) -> AccountRef? {
            model.accounts(t).first(where: match)?.ref
        }
        let scenes: [Scene] = [
            Scene(name: "claude") { m in
                m.tool = .claude
                m.selection = first(.claude) { $0.login == .loggedIn && !$0.isAPI }.map { .account($0) }
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
        ]
        let only = Set((ProcessInfo.processInfo.environment["CCA_GUI_SNAPSHOT_SCENES"] ?? "")
            .split(separator: ",").map(String.init))
        let appearances: [(String, NSAppearance.Name)] = [("light", .aqua), ("dark", .darkAqua)]
        for scene in scenes where only.isEmpty || only.contains(scene.name) {
            scene.apply(model)
            if scene.name == "routing" { model.refreshLive() }
            for (label, name) in appearances {
                NSApp.appearance = NSAppearance(named: name)
                try? await Task.sleep(nanoseconds: 900_000_000)
                await capture(window, to: dir.appendingPathComponent("\(scene.name)-\(label).png"))
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
#endif
