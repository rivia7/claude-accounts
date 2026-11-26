// 会话推来的通知（NOTIFY 帧）：和辅助程序的 cli/src/cmd/remote/local.rs（Note、NoteBudget）同样的规矩
import Foundation

/// 会话推来的一条通知：会话里的钩子（`cca remote notify`、cca 自己带的「做完了 / 在等你」）经宿主发给接着的每一处。标题、副标题、
/// 正文都是别的机器写的字：`parse` 去掉控制字符、限长度（长度和辅助程序的 `Note::parse` 一样）；显示之前 App 再清理一遍
public struct Note: Sendable, Equatable {
    public let title: String
    public let subtitle: String
    public let body: String
    /// 宿主推出去时给它的编号（16 个小写的十六进制字符）：同一条经推送（Push.swift）、经接着的会话各收到一份时照它认出来，
    /// 只弹一次。旧版的宿主推来的没有（空的）
    public let id: String

    /// local.rs 的 TITLE_MAX、BODY_MAX（按码位数）、ID_LEN
    static let titleMax = 64
    static let bodyMax = 240
    static let idLen = 16

    public init(title: String, subtitle: String = "", body: String, id: String = "") {
        self.title = title
        self.subtitle = subtitle
        self.body = body
        self.id = id
    }

    /// 帧里的 JSON（`{"title", "subtitle", "body", "id"}`）→ 通知；不是 JSON 对象、什么字都没有的不算。编号不是那个样子的当没有
    public static func parse(_ payload: Bytes) -> Note? {
        guard let d = (try? JSONSerialization.jsonObject(with: Data(payload))) as? [String: Any] else { return nil }
        let id = (d["id"] as? String).flatMap { idOK($0) ? $0 : nil } ?? ""
        let n = Note(title: clean(d["title"], titleMax), subtitle: clean(d["subtitle"], titleMax), body: clean(d["body"], bodyMax), id: id)
        return n.title.isEmpty && n.body.isEmpty ? nil : n
    }

    /// 编号的样子（local.rs 的 `Note::id_ok`）：16 个小写的十六进制字符
    public static func idOK(_ s: String) -> Bool {
        s.utf8.count == idLen && s.utf8.allSatisfy { (0x30...0x39).contains($0) || (0x61...0x66).contains($0) }
    }
}

/// 通知的限流（辅助程序的 `NoteBudget`，数也一样：host.rs 的 NOTE_BURST、NOTE_GAP）：攒得下 5 条、每 3 秒回一条。
/// 会话里的程序、别的机器一个劲地发，也只弹这么多
struct NoteBudget {
    static let burst = 5.0
    static let gap = 3.0
    private var left = NoteBudget.burst
    private var at: Double

    init(now: Double = monotonic()) {
        at = now
    }

    mutating func take(now: Double = monotonic()) -> Bool {
        left = min(left + (now - at) / Self.gap, Self.burst)
        at = now
        guard left >= 1 else { return false }
        left -= 1
        return true
    }
}
