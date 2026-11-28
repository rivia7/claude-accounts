// 推送的通知（APNs）：手机锁着屏、App 没开着时，会话里说的一条通知（cca 自己带的钩子「做完了 / 在等你」、cca remote notify）
// 经 Team 的服务器、推送网关（cca-core team-push）、Apple 的推送服务送到这台手机上。宿主把它封给这个配对（cli/src/cmd/remote/crypto.rs
// 的 note_seal，格式见 server/README.md「推送」），服务器、网关、Apple 都看不到里面；手机上解它的是通知扩展（ios/Notify），它只拿得到
// 每个配对的通知密钥（P 派生的，解不了会话、做不了配对），App 把它们存在两边共用的钥匙串分组里（NoteKeychain）。docs/dev/ios.md「推送」
import Foundation
import Security

/// 一个配对的通知密钥：HMAC(P, "cca-remote/v1/note")，加密、认证的两把再从它派生（和 crypto.rs 的 note_keys 一样）
public struct NoteKey: Sendable, Equatable {
    /// 配对编号
    public let pid: String
    /// 32 个字节
    public let raw: Bytes
    /// 那台机器叫什么（配对表里的名字：通知的副标题写「来自「…」」）
    public let name: String

    public init(pid: String, raw: Bytes, name: String) {
        self.pid = pid
        self.raw = raw
        self.name = name
    }

    /// 从配对的密钥 P 派生
    public init(pid: String, p: Bytes, name: String) {
        self.init(pid: pid, raw: hmac(p, Bytes(utf8: "cca-remote/v1/note")), name: name)
    }
}

/// 推来的一条通知：解开、清理过的（长度和 Note 一样）
public struct PushedNote: Sendable, Equatable {
    /// 哪个配对（解开它的那把通知密钥的）
    public let pid: String
    /// 那台机器叫什么（那把通知密钥记着的名字，不是通知里写的）
    public let machine: String
    /// 宿主给的编号：同一条经接着的会话也来了一份时照它认出来（Note.id）
    public let id: String
    /// 说话的会话（和 relay_list 列出来的一样）：点通知接入它
    public let sid: String
    /// 宿主推出去的时间（秒；没写是 0）
    public let at: Double
    public let title: String
    public let subtitle: String
    public let body: String
    /// 额度的通知（宿主的 status.rs 推的：额度用完了、恢复了）：这时没有会话，sid 是空的；会话里说的是 nil
    public let usage: UsageNote?
}

/// 推送的内容（`{"aps": …, "n": 封好的一份}`，通知扩展、App 拿到的 userInfo）：拿这台手机上的每把通知密钥试着解开（推送里不写
/// 配对编号：推送网关、Apple 不知道是哪个配对）。都解不开（不是这台手机的配对、取消了的、被改过、样子不对）是 nil
public func openPushed(_ userInfo: [AnyHashable: Any], keys: [NoteKey]) -> PushedNote? {
    guard let blob = userInfo["n"] as? [String: Any] else { return nil }
    for k in keys {
        if let n = noteOpen(k, blob) { return n }
    }
    return nil
}

/// 用一把通知密钥解：认证带着配对编号（别的配对的、别的密钥封的、会话的说明、账号的状态都解不开）；要有编号，会话里说的要有会话编号、
/// 标题和正文不能都是空的；额度的通知（`kind` 是 usage）要认得出是哪个账号的哪个窗口（字在这台手机上写，NoteCard）。别的 kind 不认
func noteOpen(_ key: NoteKey, _ blob: Any?) -> PushedNote? {
    guard key.raw.count == 32, let pt = unseal("cca-remote-note", SealKeys(from: Mac(key: key.raw)), [key.pid], blob), let j = parsed(pt),
          jsonInt(j["v"]) == 1, let id = j["id"] as? String, Note.idOK(id)
    else { return nil }
    let title = clean(j["title"], Note.titleMax), body = clean(j["body"], Note.bodyMax), subtitle = clean(j["subtitle"], Note.titleMax)
    let at = jsonNumber(j["at"]).flatMap { $0.isFinite ? $0 : nil } ?? 0
    switch j["kind"] as? String {
    case nil:
        guard let sid = j["sid"] as? String, sid.count == 32, Bytes.unhex(sid) != nil, !title.isEmpty || !body.isEmpty else { return nil }
        return PushedNote(pid: key.pid, machine: key.name, id: id, sid: sid, at: at, title: title, subtitle: subtitle, body: body, usage: nil)
    case "usage":
        guard let u = UsageNote(json: j) else { return nil }
        return PushedNote(pid: key.pid, machine: key.name, id: id, sid: "", at: at, title: title, subtitle: subtitle, body: body, usage: u)
    default:
        return nil
    }
}

/// 别的机器上的程序写的一行字显示之前：控制字符、看不见的排版字符去掉（行、段的分隔换成空格），去掉两头的空白，截短（按字符）
public func plainLine(_ s: String, max: Int) -> String {
    var out = String.UnicodeScalarView()
    for u in s.unicodeScalars {
        switch u.properties.generalCategory {
        case .control, .format, .surrogate, .privateUse, .unassigned: continue
        case .lineSeparator, .paragraphSeparator: out.append(" ")
        default: out.append(u)
        }
    }
    return String(String(out).trimmingCharacters(in: .whitespaces).prefix(max))
}

/// 弹出来的那一条通知上的字（App 的 Notifier、通知扩展用同一份，和 Mac App 的 RemoteNotifier 一样的规矩）：标题、副标题、正文是别的
/// 机器写的，过 `plainLine`、限长度；来自哪台机器写在副标题的开头（这台手机自己写的，对方在通知里改不了）；响哪一声（`NoteSound`）
public struct NoteCard: Sendable, Equatable {
    public static let titleMax = 80
    public static let bodyMax = 240
    /// 机器名：和配对表里的名字一样长（RemoteClient.peerName），Mac App 不另截
    public static let machineMax = 48

    public let title: String
    public let subtitle: String
    public let body: String
    public let sound: NoteSound

    /// machine：哪台机器（配对表里的名字、会话的说明里的机器名）；what：副标题里机器名后面的那一段（会话说的副标题，App 里没有时是
    /// 会话的名字）；sound：没给时照标题挑（`NoteSound(title:)`）。标题、正文都是空的是 nil
    public init?(title: String, body: String, machine: String, what: String, sound: NoteSound? = nil) {
        let t = plainLine(title, max: Self.titleMax), b = plainLine(body, max: Self.bodyMax)
        guard !t.isEmpty || !b.isEmpty else { return nil }
        let m = plainLine(machine, max: Self.machineMax), w = plainLine(what, max: Self.titleMax)
        self.title = t.isEmpty ? "远程会话" : t
        subtitle = "来自「\(m.isEmpty ? "另一台机器" : m)」" + (w.isEmpty ? "" : " · \(w)")
        self.body = b
        self.sound = sound ?? NoteSound(title: t)
    }

    /// 推来的那一条。额度的通知照它说的账号、窗口、重置时间在这里写字（时间照这台手机的时区，说法和 App 里一样：UsageText），
    /// 不用宿主写的那几句
    public init?(_ n: PushedNote, now: Double = Date().timeIntervalSince1970, calendar: Calendar = .current) {
        if let u = n.usage {
            let t = UsageText.note(u, now: now, calendar: calendar)
            self.init(title: t.title, body: t.body, machine: n.machine, what: UsageText.who(u), sound: u.event == .back ? .done : .ask)
        } else {
            self.init(title: n.title, body: n.body, machine: n.machine, what: n.subtitle)
        }
    }
}

/// 通知响哪一声：App、通知扩展里带着的两个声音文件（ios/Sounds/，ios/tools/make-sounds 合成的；系统默认的那一声又短又轻，
/// 手机放在一边时常常听不见）。一轮做完了的（标题以「做完了」结尾：cca 自己带的钩子、Codex、Antigravity 都这么写）、额度恢复了的响
/// done，别的（在等你、要你确认、在问你、额度用完了、会话里的程序自己写的）响 ask：和 Mac 上弹的（popup.rs 的 sound：做完了是 Glass，
/// 别的是 Ping）同一个分法
public enum NoteSound: String, Sendable {
    case done
    case ask

    /// 声音文件的名字（UNNotificationSound(named:) 用它）
    public var file: String { rawValue + ".caf" }

    public init(title: String) {
        self = title.hasSuffix("做完了") ? .done : .ask
    }
}

/// 通知密钥在钥匙串里：App 写、通知扩展读，放在两边共用的钥匙串分组里（`group`：两边 Info.plist 里的 CcaKeychainGroup，
/// 写成 `$(AppIdentifierPrefix)com.cca.remote.shared`，和两份 .entitlements 的 keychain-access-groups 一样）。
/// 只在这台设备上（不进 iCloud 钥匙串、不跟着备份到别的设备），开机后第一次解锁以后就读得到（锁着屏也要解得开通知）
public enum NoteKeychain {
    public static let service = "cca.remote.note"

    /// 这个程序（App 或者通知扩展）的 Info.plist 里写的共用分组
    public static var group: String? {
        Bundle.main.object(forInfoDictionaryKey: "CcaKeychainGroup") as? String
    }

    /// 存着的每一把（读不到是空的）
    public static func read(group: String?) -> [NoteKey] {
        var q: [CFString: Any] = [kSecClass: kSecClassGenericPassword, kSecAttrService: service, kSecMatchLimit: kSecMatchLimitAll,
                                  kSecReturnAttributes: true, kSecReturnData: true]
        if let group { q[kSecAttrAccessGroup] = group }
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let rows = out as? [[String: Any]] else { return [] }
        return rows.compactMap { r in
            guard let pid = r[kSecAttrAccount as String] as? String, let d = r[kSecValueData as String] as? Data, d.count == 32 else { return nil }
            return NoteKey(pid: pid, raw: Bytes(d), name: r[kSecAttrLabel as String] as? String ?? "")
        }
    }

    /// 换成这几把（和存着的一样时不动：通知扩展可能正在读）
    public static func write(_ keys: [NoteKey], group: String?) throws {
        let now = read(group: group)
        if now.count == keys.count, now.allSatisfy({ keys.contains($0) }) { return }
        var base: [CFString: Any] = [kSecClass: kSecClassGenericPassword, kSecAttrService: service]
        if let group { base[kSecAttrAccessGroup] = group }
        SecItemDelete(base as CFDictionary)
        for k in keys {
            var add = base
            add[kSecAttrAccount] = k.pid
            add[kSecAttrLabel] = k.name
            add[kSecValueData] = Data(k.raw)
            add[kSecAttrAccessible] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            add[kSecAttrSynchronizable] = false
            let st = SecItemAdd(add as CFDictionary, nil)
            guard st == errSecSuccess else { throw RemoteFailure("keychain", status: Int(st)) }
        }
    }
}
