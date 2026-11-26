import CcaRemoteKit
import Observation
import UIKit
import UserNotifications

/// 推送（APNs）：锁着屏、App 没开着时也收得到会话里的通知（做完了、在等你）。向 Apple 要这台手机的推送令牌，登记到配对的电脑所在的
/// 每台服务器上（push_set）；会话里说一条通知时，宿主把它封给这个配对，经那台服务器、cca 的推送网关、Apple 送到这台手机，通知扩展
/// （ios/Notify）拿通知密钥解开再弹出来。服务器、网关、Apple 都看不到里面。通知被关掉了（系统设置里）时把登记拿掉：服务器就不再推。
/// 「关掉的会话也推送」（设置里的开关，默认开着）关着时，登记里说只推 App 里开着的那个会话（sids），开着、关掉终端时跟着改。
/// 设置里的「额度用完、恢复时通知」（默认开着，和 Mac App 的同名开关一样的说法）：登记里说也推额度的通知（usage）：配对的电脑上一个账号的 5 小时或本周的额度用完了、到了重置的
/// 时间（恢复的那一条由服务器到点推：电脑睡着了也到）。见 docs/dev/ios.md「推送」
@MainActor
@Observable
final class Pusher {
    static let shared = Pusher()

    /// 这台手机的推送令牌（小写的十六进制）；还没拿到是 nil
    private(set) var token: String?
    /// 要令牌没成的原因（系统说的）
    private(set) var failure: String?
    /// 能不能发通知（系统设置里的）：nil 是还没看过
    private(set) var allowed: Bool?
    /// 还没接推送网关的服务器（主机名）：那里配的电脑只在 App 开着、接着会话时才收得到通知
    private(set) var unsupported: [String] = []
    /// 「关掉的会话也推送」：开着（默认）时配对的电脑上的会话都推；关着时只推 App 里开着的那个会话（锁着屏、切到别的 App 也算开着），
    /// 一个都没开着就都不推
    var closedToo = UserDefaults.standard.object(forKey: "pushClosedToo") as? Bool ?? true {
        didSet {
            UserDefaults.standard.set(closedToo, forKey: "pushClosedToo")
            sync()
        }
    }
    /// 不认这个开关的服务器（主机名，旧版）：经它配对的电脑，关掉的会话照样推（开关关着时设置里说一声）
    private(set) var unfiltered: [String] = []
    /// 「额度用完、恢复时通知」：配对的电脑上的账号额度用完了、恢复了时推一条
    var usageNotes = UserDefaults.standard.object(forKey: "pushUsage") as? Bool ?? true {
        didSet {
            UserDefaults.standard.set(usageNotes, forKey: "pushUsage")
            sync()
        }
    }
    /// 推不了额度的通知的服务器（主机名，旧版）：开关开着时设置里说一声
    private(set) var usageless: [String] = []

    @ObservationIgnored private weak var model: AppModel?
    @ObservationIgnored private var syncing = false
    @ObservationIgnored private var again = false

    #if DEBUG
    /// TestDriver 用：不向 Apple 要令牌（用它给的）、当作允许发通知、不弹「能不能发通知」，每台服务器登记的结果交给 spy
    @ObservationIgnored var testing = false
    @ObservationIgnored var spy: ((String) -> Void)?
    #endif

    /// 推送服务的环境：描述文件（embedded.mobileprovision）里写的 aps-environment，development 的推送服务叫 sandbox；
    /// App Store、TestFlight 装的是 production；模拟器是 sandbox
    static let env: String = {
        #if targetEnvironment(simulator)
        return "sandbox"
        #else
        guard let url = Bundle.main.url(forResource: "embedded", withExtension: "mobileprovision"), let d = try? Data(contentsOf: url),
              let s = String(data: d, encoding: .isoLatin1), let k = s.range(of: "<key>aps-environment</key>")
        else { return "production" }
        let rest = s[k.upperBound...].prefix(80)
        return rest.contains("<string>development</string>") ? "sandbox" : "production"
        #endif
    }()

    /// 启动时（演示模式不管）：通知扩展的通知密钥照配对表重写一遍、向 Apple 要令牌（拿到以后 `got`）、有配对过的电脑时问一次能不能发通知
    func start(_ m: AppModel) {
        model = m
        guard !m.isDemo else { return }
        m.backend.syncNoteKeys()
        #if DEBUG
        if testing { return sync() }
        #endif
        UIApplication.shared.registerForRemoteNotifications()
        askIfNeeded()
    }

    func got(_ deviceToken: Data) {
        token = deviceToken.map { String(format: "%02x", $0) }.joined()
        failure = nil
        sync()
    }

    func failed(_ e: Error) {
        failure = e.localizedDescription
    }

    #if DEBUG
    func testToken(_ hex: String) {
        token = hex
        sync()
    }
    #endif

    /// 配好了一台（这台服务器上以前登记的不算数了：配对是新的）、取消了一个配对以后
    func peersChanged(on server: ServerRef) {
        guard let model, !model.isDemo else { return }
        model.backend.syncNoteKeys()
        UserDefaults.standard.removeObject(forKey: memoKey(server))
        askIfNeeded()
        sync()
    }

    /// App 里开着的会话变了（开了、关了一个终端）：只推开着的会话时，登记跟着改
    func openChanged() {
        if !closedToo { sync() }
    }

    /// 有配对过的电脑、还没问过时问一次能不能发通知（问过了系统记着，不再问）
    func askIfNeeded() {
        guard let model, !model.isDemo, !model.peers.isEmpty else { return }
        #if DEBUG
        if testing { return }
        #endif
        Task {
            let center = UNUserNotificationCenter.current()
            guard await center.notificationSettings().authorizationStatus == .notDetermined else { return }
            _ = try? await center.requestAuthorization(options: [.alert, .sound])
            sync()
        }
    }

    /// 每台服务器上登记的和该登记的对一对，不一样的才发（App 回到前台、拿到令牌、配对变了时）
    func sync() {
        guard let model, !model.isDemo else { return }
        if syncing {
            again = true
            return
        }
        syncing = true
        let backend = model.backend
        Task {
            await syncNow(backend)
            syncing = false
            if again {
                again = false
                sync()
            }
        }
    }

    /// 记着的：一台服务器上登记的是什么（"令牌 环境"，要额度的通知时后面是 " usage"，只推开着的会话时再后面是 " only:会话编号,…"；
    /// 拿掉了是空的）、什么时候登记的、它是不是说过还没接推送网关、是不是不认只推哪几个会话、是不是推不了额度的通知（旧版；以前的版本
    /// 记的没有后两项）
    private struct Memo: Codable {
        let reg: String
        let at: Double
        let unsupported: Bool
        var unfiltered: Bool? = nil
        var usageless: Bool? = nil
    }

    private func memoKey(_ s: ServerRef) -> String { "push " + s.url }

    private func memo(_ s: ServerRef) -> Memo? {
        UserDefaults.standard.data(forKey: memoKey(s)).flatMap { try? JSONDecoder().decode(Memo.self, from: $0) }
    }

    private func remember(_ s: ServerRef, _ m: Memo) {
        UserDefaults.standard.set(try? JSONEncoder().encode(m), forKey: memoKey(s))
    }

    /// 系统设置里能不能发通知（测试时当作能）
    private func status() async -> UNAuthorizationStatus {
        #if DEBUG
        if testing { return .authorized }
        #endif
        return await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
    }

    /// App 里开着的、在这台服务器上的会话（只推开着的会话时登记的就是它们）
    private func openSids(on s: ServerRef) -> [String] {
        guard let t = model?.terminal, t.peer.server.url == s.url else { return [] }
        return [t.session.sid]
    }

    private func syncNow(_ backend: Backend) async {
        let ok = await status()
        let can = ok == .authorized || ok == .provisional || ok == .ephemeral
        if ok != .notDetermined { allowed = can }
        // 该登记的：能发通知、拿到了令牌时是令牌和环境，不然什么都不登记（拿掉）
        let reg: (token: String, env: String)? = can ? token.map { ($0, Self.env) } : nil
        // 额度的通知：登记时总说要不要（关着也说：服务器照它不推）
        let usage: Bool? = reg == nil ? nil : usageNotes
        let base = reg.map { "\($0.token) \($0.env)" + (usageNotes ? " usage" : "") } ?? ""
        let now = Date().timeIntervalSince1970
        var notYet = [String](), notOnly = [String](), notUsage = [String]()
        for s in backend.servers() {
            let m = memo(s)
            if let m, m.unsupported, now - m.at < 86400 {
                notYet.append(s.host)
                continue
            }
            // 关掉的会话不推：只推开着的（这台服务器上一个都没开着是空的）
            let sids: [String]? = reg != nil && !closedToo ? openSids(on: s) : nil
            let want = base + (sids.map { " only:" + $0.joined(separator: ",") } ?? "")
            // 推不了额度的通知的服务器（旧版）：一天以后再登记一次，看它更新了没有
            let usageless = usage == true && m?.usageless == true
            // 不认 sids 的服务器（旧版，照旧都推）：开着的会话变了不用再登记，一天以后再看它更新了没有
            if sids != nil, let m, m.unfiltered == true, m.reg == base, now - m.at < 86400 {
                notOnly.append(s.host)
                if usageless { notUsage.append(s.host) }
                continue
            }
            // 一样的过一周再登记一次（服务器那边可能丢了：推送服务说令牌没用了、服务器换过）
            if let m, !m.unsupported, m.reg == want, now - m.at < (usageless ? 86400 : 7 * 86400) {
                if usageless { notUsage.append(s.host) }
                continue
            }
            if m == nil, want.isEmpty { continue }   // 没登记过：没什么可拿掉的
            let r = await backend.pushSet(on: s, token: reg?.token ?? "", env: reg?.env ?? Self.env, sids: sids, usage: usage)
            switch r {
            case .ok(let filtered, let usageOK):
                let noUsage = usage == true && !usageOK
                #if DEBUG
                spy?("push \(s.host) ok" + (sids == nil ? "" : filtered ? " only" : " all") + (usage == true ? (usageOK ? " usage" : " no-usage") : ""))
                #endif
                if sids == nil || filtered {
                    remember(s, Memo(reg: want, at: now, unsupported: false, usageless: noUsage))
                } else {
                    remember(s, Memo(reg: base, at: now, unsupported: false, unfiltered: true, usageless: noUsage))
                    notOnly.append(s.host)
                }
                if noUsage { notUsage.append(s.host) }
            case .failed(let f):
                #if DEBUG
                spy?("push \(s.host) \(f.code)")
                #endif
                if f.code == "no_push" {
                    remember(s, Memo(reg: "", at: now, unsupported: true))
                    notYet.append(s.host)
                }
                // 别的（没问到、服务器这会儿忙）：下次再说
            }
        }
        unsupported = notYet
        unfiltered = notOnly
        usageless = notUsage
    }
}

/// 推送令牌经 UIApplicationDelegate 来（SwiftUI 的 App 没有这两个回调）
@MainActor
final class AppDelegate: NSObject, UIApplicationDelegate {
    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        Pusher.shared.got(deviceToken)
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        Pusher.shared.failed(error)
    }
}
