import CcaRemoteKit
import UserNotifications

/// 通知扩展（com.cca.remote.notify）：推来的通知（APNs，mutable-content）弹出来之前先到这里。推送里只有一句「远程会话里有新的通知」
/// 和封好的一份（`n`）：拿这台手机上每个配对的通知密钥（App 存在两边共用的钥匙串分组里，NoteKeychain）试着解开，换上真的标题、
/// 副标题、正文和提示音（CcaRemoteKit 的 NoteCard：和 App 自己弹的一样），同一个会话的叠在一起，点它回到那个会话（App 的 Notifier）。
/// 额度的通知（那台电脑上一个账号的额度用完了、恢复了）照这台手机的时区写字，同一个账号的叠在一起，点它打开那个账号的那一页。
/// 解不开的（取消了的配对、这台手机没有的、被改过的）照推送里写的那句弹。这里拿不到配对的密钥、连不了服务器，也不存任何东西。
/// 见 docs/dev/ios.md「推送」
final class NotificationService: UNNotificationServiceExtension {
    private var deliver: ((UNNotificationContent) -> Void)?
    private var original: UNNotificationContent?

    override func didReceive(_ request: UNNotificationRequest, withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void) {
        deliver = contentHandler
        original = request.content
        guard let content = request.content.mutableCopy() as? UNMutableNotificationContent else { return finish(request.content) }
        if let n = openPushed(request.content.userInfo, keys: NoteKeychain.read(group: NoteKeychain.group)), let card = NoteCard(n) {
            content.title = card.title
            content.subtitle = card.subtitle
            content.body = card.body
            content.sound = UNNotificationSound(named: UNNotificationSoundName(rawValue: card.sound.file))
            // 封着的那一份不留在通知里；App 点开时、在前台去重时用这几项（Notifier）
            if let u = n.usage {
                content.threadIdentifier = "usage:\(n.pid):\(u.account)"
                content.userInfo = ["peer": n.pid, "account": u.account, "note": n.id]
            } else {
                content.threadIdentifier = n.sid
                content.userInfo = ["peer": n.pid, "sid": n.sid, "note": n.id]
            }
        }
        finish(content)
    }

    /// 系统给的时间快到了（解一份用不了几毫秒，不该走到这里）：照推送里写的那句弹
    override func serviceExtensionTimeWillExpire() {
        if let original { finish(original) }
    }

    private func finish(_ content: UNNotificationContent) {
        deliver?(content)
        deliver = nil
    }
}
