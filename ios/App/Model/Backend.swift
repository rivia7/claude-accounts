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
    func link(peer: Peer, session: RemoteSession, cols: Int, rows: Int, readOnly: Bool) -> TerminalLink
    /// 这台手机在这台服务器上的指纹（电脑那头 `cca remote peers` 看得到同一个）
    func fingerprint(on server: ServerRef) -> String?
}

/// 接着的一个会话（Attachment 或者演示的那种）：事件在主线程上出来
@MainActor
protocol TerminalLink: AnyObject {
    var onOutput: (Bytes) -> Void { get set }
    var onState: (Attachment.State) -> Void { get set }
    var onExit: (Int32) -> Void { get set }
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

    func link(peer: Peer, session: RemoteSession, cols: Int, rows: Int, readOnly: Bool) -> TerminalLink {
        Attachment(client: client, peer: peer, sid: session.sid, cols: cols, rows: rows, readOnly: readOnly || session.info.viewOnly)
    }

    func fingerprint(on server: ServerRef) -> String? { client.fingerprint(on: server) }
}

/// 密钥（这台手机在每台服务器上的身份、每个配对的密钥）存钥匙串：只在这台设备上（不进 iCloud 钥匙串、不跟着备份到别的设备），
/// 开机后第一次解锁以后才读得到。配对表（不是秘密：机器名、服务器地址和指纹）存在 App 自己的目录里。
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
