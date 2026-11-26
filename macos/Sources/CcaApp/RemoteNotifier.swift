import AppKit
import UserNotifications

/// 别的机器上的会话推来的通知（PROTOCOL 9.3）：会话里的钩子 `cca remote notify`，或者会话里的程序自己写的 OSC 777 / OSC 9，
/// 经接入端写进这个终端。用户没在看这块终端时弹一条 macOS 通知；点它回到 App、换到这个终端。
///
/// 字是别的机器写的：标题、正文过 `Texts.plainLine`、限长度；来自哪台机器写在副标题里（App 自己写的，对方改不了）；
/// 一个终端几秒钟最多一条（`RemoteNotifier.allow`）。通知里没有终端的其它内容。
@MainActor
enum RemoteNotifier {
    /// 点通知时要换到的终端在哪个模型里（CcaApp 启动时设）
    static weak var model: AppModel?
    private static let delegate = Delegate()
    private static var asked = false
    private static var last: [UUID: Date] = [:]

    /// 同一个终端两条之间至少隔这么久（cca 那边已经限过一次流：这里再守一道）
    static let gap: TimeInterval = 3

    static func allow(_ id: UUID, now: Date = Date()) -> Bool {
        if let t = last[id], now.timeIntervalSince(t) < gap { return false }
        last[id] = now
        return true
    }

    /// 通知中心的 delegate 只有一个：会话的通知和额度的通知（UsageNotifier）共用这一个（App 启动时就设好：点通知把 App 打开时也认得）
    static func installDelegate() {
        UNUserNotificationCenter.current().delegate = delegate
    }

    static func post(title: String, body: String, from terminal: RemoteTerminal) {
        let title = Texts.plainLine(title, max: 80), body = Texts.plainLine(body, max: 240)
        guard !title.isEmpty || !body.isEmpty, allow(terminal.id) else { return }
        let center = UNUserNotificationCenter.current()
        center.delegate = delegate
        let content = UNMutableNotificationContent()
        content.title = title.isEmpty ? "远程会话" : title
        let s = terminal.session
        content.subtitle = "来自「\(nonEmpty(s.machine) ?? "另一台机器")」 · " + (nonEmpty(Texts.remoteTitle(s)) ?? s.short)
        content.body = body
        content.sound = .default
        content.userInfo = ["terminal": terminal.id.uuidString]
        let request = UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil)
        let send = { center.add(request) { _ in } }
        if asked { send(); return }
        asked = true
        center.requestAuthorization(options: [.alert, .sound]) { ok, _ in
            if ok { DispatchQueue.main.async { send() } }
        }
    }

    /// 点了通知：回到 App，换到那个终端（它已经关掉了就是远程会话那一页）
    fileprivate static func open(_ id: String?) {
        WindowOpener.shared.showMain()
        guard let model else { return }
        model.selection = .remoteSessions
        if let id, let t = model.remoteTerminals.first(where: { $0.id.uuidString == id }) {
            model.showRemoteTerminal(t)
        }
    }

    private final class Delegate: NSObject, UNUserNotificationCenterDelegate, @unchecked Sendable {
        // App 在最前面时也弹（用户可能正看着别的页）
        func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                    withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) {
            done([.banner, .sound])
        }

        // 点了通知：会话的回到那个终端，额度的（UsageNotifier）打开那个账号
        func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                    withCompletionHandler done: @escaping () -> Void) {
            let info = response.notification.request.content.userInfo
            let id = info["terminal"] as? String
            let tool = info["tool"] as? String, account = info["account"] as? String
            DispatchQueue.main.async {
                if let tool, let account {
                    UsageNotifier.open(tool: tool, key: account)
                } else {
                    RemoteNotifier.open(id)
                }
                done()
            }
        }
    }
}
