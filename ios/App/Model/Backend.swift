import CcaRemoteKit
import Foundation
import Security

/// 界面背后的那一头：真的（RemoteClient，连服务器）或者演示（DemoBackend，本地模拟，不联网）
@MainActor
protocol Backend: AnyObject {
    var isDemo: Bool { get }
    func peers() -> [Peer]
    func list() async -> [PeerListing]
    func pair(_ invite: PairInvite, progress: @escaping @Sendable (PairStep) -> Void) async throws -> Peer
    @discardableResult func unpair(_ peer: Peer) async -> Bool
    /// fresh：刚远程启动的会话（服务器上可能还没有它：Attachment 的 fresh）
    func link(peer: Peer, session: RemoteSession, cols: Int, rows: Int, readOnly: Bool, fresh: Bool) -> TerminalLink
    /// 远程启动（CcaRemoteKit 的 Launch.swift）：请那台电脑的启动器照模板起一个会话，等它回话。account：挑的账号（说明里能挑的那几个
    /// 之一）；旧版的 cca 的说明里没有能挑的账号，是 nil（用模板自己定的账号）
    func launch(peer: Peer, launcher: HostLauncher, template: LaunchTemplate, account: LaunchAccount?, cols: Int, rows: Int,
                line: String?) async -> LaunchResult
    /// 这台手机在这台服务器上的指纹（电脑那头 `cca remote peers` 看得到同一个）
    func fingerprint(on server: ServerRef) -> String?
    /// 配对的电脑所在的服务器（每台一次）
    func servers() -> [ServerRef]
    /// 在这台服务器上登记推送（Pusher）；sids：只推这几个会话，nil 是都推；usage：也推额度的通知，nil 是不说
    func pushSet(on server: ServerRef, token: String, env: String, sids: [String]?, usage: Bool?) async -> PushSetResult
    /// 通知扩展用的通知密钥照配对表重写一遍（一样就不动）
    func syncNoteKeys()
}

/// 接着的一个会话（Attachment 或者演示的那种）：事件在主线程上出来
@MainActor
protocol TerminalLink: AnyObject {
    var onOutput: (Bytes) -> Void { get set }
    /// 会话现在的大小（新的宿主说的；演示的不说）
    var onSize: (Int, Int) -> Void { get set }
    var onState: (Attachment.State) -> Void { get set }
    var onExit: (Int32) -> Void { get set }
    /// 会话推来的通知（演示的没有）
    var onNote: (Note) -> Void { get set }
    func start()
    func nudge()
    @discardableResult func input(_ data: Bytes) -> Bool
    func resize(cols: Int, rows: Int)
    func detach()
}

extension Attachment: TerminalLink {}

/// 真的那一头：CcaRemoteKit 的 RemoteClient，密钥存钥匙串
@MainActor
final class LiveBackend: Backend {
    let client: RemoteClient

    init(deviceName: String) {
        client = RemoteClient(store: KeychainStore.shared, deviceName: deviceName)
    }

    var isDemo: Bool { false }
    func peers() -> [Peer] { client.peers() }
    func list() async -> [PeerListing] { await client.list() }

    func pair(_ invite: PairInvite, progress: @escaping @Sendable (PairStep) -> Void) async throws -> Peer {
        try await client.pair(invite, progress: progress)
    }

    func unpair(_ peer: Peer) async -> Bool { await client.unpair(peer) }

    func link(peer: Peer, session: RemoteSession, cols: Int, rows: Int, readOnly: Bool, fresh: Bool) -> TerminalLink {
        Attachment(client: client, peer: peer, sid: session.sid, cols: cols, rows: rows, readOnly: readOnly || session.info.viewOnly, fresh: fresh)
    }

    func launch(peer: Peer, launcher: HostLauncher, template: LaunchTemplate, account: LaunchAccount?, cols: Int, rows: Int,
                line: String?) async -> LaunchResult {
        await client.launch(peer, launcher: launcher.sid, template: template, cols: cols, rows: rows, line: line, account: account?.id)
    }

    func fingerprint(on server: ServerRef) -> String? { client.fingerprint(on: server) }

    func servers() -> [ServerRef] { client.servers() }

    func pushSet(on server: ServerRef, token: String, env: String, sids: [String]?, usage: Bool?) async -> PushSetResult {
        await client.pushSet(on: server, token: token, env: env, sids: sids, usage: usage)
    }

    /// 存进 App 和通知扩展共用的钥匙串分组（NoteKeychain）：通知扩展只拿得到这几把，拿不到配对的密钥
    func syncNoteKeys() {
        try? NoteKeychain.write(client.noteKeys(), group: NoteKeychain.group)
    }
}

/// 密钥（这台手机在每台服务器上的身份、每个配对的密钥）存钥匙串：只在这台设备上（不进 iCloud 钥匙串、不跟着备份到别的设备），
/// 开机后第一次解锁以后才读得到。配对表（不是秘密：机器名、服务器地址和指纹）存在 App 自己的目录里。这些都在 App 自己的钥匙串分组里
/// （默认的那个：.entitlements 里 keychain-access-groups 的第一个）；通知扩展读的通知密钥另在两边共用的分组里（NoteKeychain）。
/// 钥匙串里的东西删掉 App 以后还在：重新装的第一次打开时清掉（配对表已经没了，留着的密钥用不上）
final class KeychainStore: RemoteStore, @unchecked Sendable {
    static let shared = KeychainStore()
    private let service = "cca.remote"
    private let lock = NSLock()

    private var peersURL: URL {
        let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("peers.json")
    }

    private init() {
        let d = UserDefaults.standard
        if !d.bool(forKey: "installed") {
            SecItemDelete([kSecClass: kSecClassGenericPassword, kSecAttrService: service] as CFDictionary)
            SecItemDelete([kSecClass: kSecClassGenericPassword, kSecAttrService: NoteKeychain.service] as CFDictionary)
            d.set(true, forKey: "installed")
        }
    }

    func secret(_ key: String) -> Bytes? {
        let q: [CFString: Any] = [kSecClass: kSecClassGenericPassword, kSecAttrService: service, kSecAttrAccount: key,
                                  kSecReturnData: true, kSecMatchLimit: kSecMatchLimitOne]
        var out: CFTypeRef?
        guard lock.withLock({ SecItemCopyMatching(q as CFDictionary, &out) }) == errSecSuccess, let d = out as? Data else { return nil }
        return Bytes(d)
    }

    func setSecret(_ key: String, _ value: Bytes?) throws {
        let base: [CFString: Any] = [kSecClass: kSecClassGenericPassword, kSecAttrService: service, kSecAttrAccount: key]
        try lock.withLock {
            SecItemDelete(base as CFDictionary)
            guard let value else { return }
            var add = base
            add[kSecValueData] = Data(value)
            add[kSecAttrAccessible] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            add[kSecAttrSynchronizable] = false
            let st = SecItemAdd(add as CFDictionary, nil)
            guard st == errSecSuccess else { throw RemoteFailure("keychain", status: Int(st)) }
        }
    }

    func peersData() -> Data? {
        try? Data(contentsOf: peersURL)
    }

    func setPeersData(_ data: Data) throws {
        try data.write(to: peersURL, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }
}
