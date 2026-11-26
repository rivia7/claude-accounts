import CcaRemoteKit
import Foundation
import UserNotifications

/// 会话里的通知弹成这台手机的通知，点它回到那个会话的终端。两条路来：
/// - 接着的会话推来的（宿主转来的 NOTIFY：会话里的钩子，cca 自己带的「做完了 / 在等你」、`cca remote notify`；或者会话里的程序自己写的
///   OSC 777 / OSC 9）：这里弹（`post`）
/// - 推送（APNs，Pusher）：锁着屏、App 没开着时也到；通知扩展（ios/Notify）解开、改好字再弹，App 在前台时经这里的 delegate
/// 同一条两条路都来了（App 开着、接着那个会话时）：照宿主给的编号（Note.id）只弹一次，先到的那一份算数。
/// 字的规矩和 Mac App 的 RemoteNotifier（PROTOCOL 9.3）一样（CcaRemoteKit 的 NoteCard）：字是别的机器写的，标题、正文清理过、限长度；
/// 来自哪台机器写在副标题的开头（App 自己写的，对方改不了）；一个终端几秒钟最多一条（CcaRemoteKit 已经限过一次流：这里再守一道）。
/// 通知里没有终端的其它内容。
///
/// 和 Mac 不一样的一处：App 在前台、正开着这个终端时也弹（横幅、声音）。手机放在一边等它做完时正要它响
@MainActor
enum Notifier {
    /// 点通知时要回到的会话在哪个模型里（CcaApp 启动时设）
    private static weak var model: AppModel?
    private static let delegate = Delegate()
    private static var last: [UUID: Date] = [:]
    /// 弹过的通知的编号（宿主给的）和什么时候：同一条经另一条路又来了不再弹
    private static var shown: [String: Date] = [:]

    /// 同一个终端两条之间至少隔这么久（和 Mac App 一样）
    static let gap: TimeInterval = 3
    /// 编号记多久（推送一般几秒内就到；推送服务替锁着的手机攒着的，过了这么久再到的也不至于和接着的那份撞上）
    static let remember: TimeInterval = 600

    #if DEBUG
    /// TestDriver 用：要弹的标题、副标题、正文交给它（不真的弹、不要权限）
    static var spy: ((String, String, String) -> Void)?
    #endif

    /// App 启动时调一次：点通知（App 没在运行时点的也算）经这里的 delegate，得在启动完之前设好
    static func start(_ m: AppModel) {
        model = m
        UNUserNotificationCenter.current().delegate = delegate
    }

    static func allow(_ id: UUID, now: Date = Date()) -> Bool {
        if let t = last[id], now.timeIntervalSince(t) < gap { return false }
        last[id] = now
        return true
    }

    /// 这个编号的通知还没弹过：记下（以后再来就不弹）。没有编号的（旧版的宿主、会话里的程序自己写的）总是弹
    static func firstTime(_ id: String, now: Date = Date()) -> Bool {
        guard !id.isEmpty else { return true }
        shown = shown.filter { now.timeIntervalSince($0.value) < remember }
        guard shown[id] == nil else { return false }
        shown[id] = now
        return true
    }

    static func post(_ n: Note, from t: TerminalSession) {
        let machine = Texts.nonEmpty(t.session.info.machine) ?? t.peer.name
        let what = Texts.nonEmpty(Texts.plainLine(n.subtitle, max: NoteCard.titleMax)) ?? Texts.sessionTitle(t.session)
        guard let card = NoteCard(title: n.title, body: n.body, machine: machine, what: what), allow(t.id), firstTime(n.id) else { return }
        let c = Card(id: n.id, title: card.title, subtitle: card.subtitle, body: card.body, peer: t.peer.pid, sid: t.session.sid,
                     readOnly: t.readOnly)
        #if DEBUG
        if let spy {
            spy(c.title, c.subtitle, c.body)
            return
        }
        #endif
        Task {
            // 没问过能不能发通知：这时问（用户知道是为了什么）。配好第一台电脑时 Pusher 已经问过一次
            let center = UNUserNotificationCenter.current()
            switch await center.notificationSettings().authorizationStatus {
            case .notDetermined:
                if (try? await center.requestAuthorization(options: [.alert, .sound])) == true { send(c) }
                Pusher.shared.sync()
            case .denied:
                break
            default:
                send(c)
            }
        }
    }

    /// 一条要弹的通知（只有字：在通知中心的回调里再做成 UNNotificationRequest）
    private struct Card: Sendable {
        let id, title, subtitle, body, peer, sid: String
        let readOnly: Bool
    }

    private nonisolated static func send(_ c: Card) {
        let content = UNMutableNotificationContent()
        content.title = c.title
        content.subtitle = c.subtitle
        content.body = c.body
        content.sound = .default
        content.threadIdentifier = c.sid   // 同一个会话的叠在一起（推来的也是这样）
        content.userInfo = ["peer": c.peer, "sid": c.sid, "view": c.readOnly, "note": c.id]
        // 标识用宿主给的编号：推送服务照同一个编号合并（apns-collapse-id），万一两份都进了通知中心也只留一条
        let id = c.id.isEmpty ? UUID().uuidString : c.id
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: id, content: content, trigger: nil))
    }

    private final class Delegate: NSObject, UNUserNotificationCenterDelegate, @unchecked Sendable {
        // App 在前台时也弹（见上）；推来的（通知扩展解开以后的）这条接着的会话已经弹过了就不再弹
        func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                    withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) {
            let pushed = notification.request.trigger is UNPushNotificationTrigger
            let id = notification.request.content.userInfo["note"] as? String ?? ""
            DispatchQueue.main.async {
                if pushed, !Notifier.firstTime(id) { return done([]) }
                done([.banner, .list, .sound])
            }
        }

        // 点了通知：回到那个会话的终端（推来的、通知扩展没解开的没有这两项：只是打开 App）
        func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                    withCompletionHandler done: @escaping () -> Void) {
            let info = response.notification.request.content.userInfo
            let pid = info["peer"] as? String, sid = info["sid"] as? String, view = info["view"] as? Bool ?? false
            DispatchQueue.main.async {
                if let pid, let sid { Notifier.model?.reopen(peer: pid, sid: sid, readOnly: view) }
                done()
            }
        }
    }
}
