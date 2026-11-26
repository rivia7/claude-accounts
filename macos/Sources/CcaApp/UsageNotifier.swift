import AppKit
import Observation
import UserNotifications

/// 额度用完、恢复的通知（设置里的「额度用完、恢复时通知」，默认开着）：照 CcaKit 的 UsageAlerts，账号的 5 小时或本周的额度用完了
/// 马上弹一条（「额度用完了」「5 小时的额度用完了，16:00 恢复」），同时定好重置的时间交给系统，到点弹「额度恢复了」（App 那时没在跑
/// 也到）。点它打开那个账号。弹过的记在 App 的设置里（同一轮只弹一次），演示模式不弹。和 cca 推给配对的手机的是同一个规则、同样的字
/// （docs/dev/push.md「额度的通知」）
@MainActor
@Observable
final class UsageNotifier {
    static let shared = UsageNotifier()

    /// 设置里的开关；关掉时把定好的「额度恢复了」也撤掉
    var enabled: Bool = UserDefaults.standard.object(forKey: UsageNotifier.enabledKey) as? Bool ?? true {
        didSet {
            UserDefaults.standard.set(enabled, forKey: Self.enabledKey)
            if enabled {
                check()
            } else {
                withdrawPending()
            }
        }
    }

    @ObservationIgnored weak var model: AppModel?
    @ObservationIgnored private var alerts: UsageAlerts?
    @ObservationIgnored private var asked = false

    private static let enabledKey = "usage.notes"
    private static let memoKey = "usage.notes.memo"
    private static let prefix = "cca.usage."

    /// 账号、用量可能变了（AppModel 的 hooks.accountsChanged）：照现在的用量看要不要弹
    func check() {
        guard enabled, let model, !model.isFixtureMode, model.phase == .ready else { return }
        let now = Date().timeIntervalSince1970
        var a = alerts ?? Self.load()
        let notes = a.update(model.usageAlertItems(), now: now)
        if a != alerts {
            alerts = a
            Self.save(a)
        }
        guard !notes.isEmpty else { return }
        let requests = notes.flatMap { n -> [UNNotificationRequest] in
            [Self.request(n, now: now, at: nil)] + (n.back(now: now).map { [Self.request($0, now: now, at: $0.resetsEpoch)] } ?? [])
        }
        let center = UNUserNotificationCenter.current()
        RemoteNotifier.installDelegate()
        let send = { for r in requests { center.add(r) { _ in } } }
        if asked { return send() }
        asked = true
        center.requestAuthorization(options: [.alert, .sound]) { ok, _ in
            if ok { DispatchQueue.main.async { send() } }
        }
    }

    /// 一条通知：at 是到点再弹的时间（「额度恢复了」），nil 是马上弹
    private static func request(_ n: UsageAlerts.Note, now: Double, at: Double?) -> UNNotificationRequest {
        let t = Texts.usageNote(n, now: now)
        let content = UNMutableNotificationContent()
        content.title = t.title
        content.subtitle = t.subtitle
        content.body = t.body
        content.sound = .default
        content.threadIdentifier = "usage:" + n.account.description   // 同一个账号的叠在一起
        content.userInfo = ["tool": n.account.tool.rawValue, "account": n.account.key]
        let trigger = at.map { UNTimeIntervalNotificationTrigger(timeInterval: max($0 - now, 1), repeats: false) }
        return UNNotificationRequest(identifier: n.identifier, content: content, trigger: trigger)
    }

    /// 关掉开关时：定好还没弹的「额度恢复了」撤掉
    private func withdrawPending() {
        let center = UNUserNotificationCenter.current(), prefix = Self.prefix
        center.getPendingNotificationRequests { reqs in
            let ids = reqs.map(\.identifier).filter { $0.hasPrefix(prefix) }
            if !ids.isEmpty { center.removePendingNotificationRequests(withIdentifiers: ids) }
        }
    }

    /// 点了通知：回到 App，打开那个账号
    static func open(tool: String, key: String) {
        WindowOpener.shared.showMain()
        guard let model = shared.model else { return }
        let ref = AccountRef(tool: ToolID(rawValue: tool), key: key)
        if model.account(ref) != nil { model.selection = .account(ref) }
    }

    private static func load() -> UsageAlerts {
        UserDefaults.standard.data(forKey: memoKey).flatMap { try? JSONDecoder().decode(UsageAlerts.self, from: $0) } ?? UsageAlerts()
    }

    private static func save(_ a: UsageAlerts) {
        UserDefaults.standard.set(try? JSONEncoder().encode(a), forKey: memoKey)
    }
}
