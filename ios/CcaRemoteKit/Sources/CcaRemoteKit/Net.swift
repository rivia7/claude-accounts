// 和服务器说话：签了名的请求（HTTPS，证书照二维码里的指纹钉住，和 cca 的设备一样）、进数据端口的 TCP 连接。
import CryptoKit
import Foundation
import Network
import Security

/// 出了什么事：界面照 code 说话。code 是服务器说的 error（`^[a-z_]{1,32}$`），或者这边的：unreachable（没问到）、
/// pin（证书和二维码里的指纹对不上）、code（配对码不对）、no_pairing、no_answer、bad_peer、error
public struct RemoteFailure: Error, Equatable, Sendable {
    public let code: String
    /// 服务器回的状态码（没问到是 0）
    public let status: Int

    public init(_ code: String, status: Int = 0) {
        self.code = code
        self.status = status
    }
}

// MARK: - 证书的公钥指纹

/// DER 的一项：(标签, 内容的范围, 整项的范围)
private func tlv(_ b: Bytes, _ at: Int) -> (tag: UInt8, body: Range<Int>, whole: Range<Int>)? {
    guard at + 2 <= b.count else { return nil }
    let tag = b[at]
    var len = Int(b[at + 1])
    var head = 2
    if len & 0x80 != 0 {
        let n = len & 0x7F
        guard (1...4).contains(n), at + 2 + n <= b.count else { return nil }
        len = 0
        for i in 0..<n {
            len = len << 8 | Int(b[at + 2 + i])
        }
        head += n
    }
    guard at + head + len <= b.count else { return nil }
    return (tag, at + head..<at + head + len, at..<at + head + len)
}

/// 证书（DER）里的 SubjectPublicKeyInfo（整项）：curl 的 --pinnedpubkey 算的就是它的 SHA-256
func spki(ofCertificate der: Bytes) -> Bytes? {
    guard let cert = tlv(der, 0), cert.tag == 0x30, let tbs = tlv(der, cert.body.lowerBound), tbs.tag == 0x30 else { return nil }
    var at = tbs.body.lowerBound
    var fields = [(tag: UInt8, body: Range<Int>, whole: Range<Int>)]()
    while at < tbs.body.upperBound, let f = tlv(der, at) {
        fields.append(f)
        at = f.whole.upperBound
    }
    // [0] 版本（可以没有）、序列号、签名算法、签发者、有效期、主体、公钥
    let rest = fields.first?.tag == 0xA0 ? Array(fields.dropFirst()) : fields
    guard rest.count >= 6, rest[5].tag == 0x30 else { return nil }
    return Bytes(der[rest[5].whole])
}

func pinOf(certificate der: Bytes) -> String? {
    spki(ofCertificate: der).map { "sha256//" + sha256($0).b64 }
}

/// HTTPS：有指纹时只认指纹对得上的证书（自签的，不看证书机构、名字），没有时照系统的证书机构验
final class PinnedDelegate: NSObject, URLSessionDelegate, @unchecked Sendable {
    let pins: [String]
    /// 上一次握手时证书对不上（给出 pin 这个原因，不笼统地说连不上）
    private let lock = NSLock()
    private var mismatch = false

    init(pins: [String]) {
        self.pins = pins
    }

    var pinFailed: Bool {
        lock.lock()
        defer { lock.unlock() }
        return mismatch
    }

    func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge,
                    completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let trust = challenge.protectionSpace.serverTrust, !pins.isEmpty
        else { return completionHandler(.performDefaultHandling, nil) }
        let chain = (SecTrustCopyCertificateChain(trust) as? [SecCertificate]) ?? []
        if let leaf = chain.first, let pin = pinOf(certificate: Bytes(SecCertificateCopyData(leaf) as Data)), pins.contains(pin) {
            return completionHandler(.useCredential, URLCredential(trust: trust))
        }
        lock.lock()
        mismatch = true
        lock.unlock()
        completionHandler(.cancelAuthenticationChallenge, nil)
    }
}

/// 发签了名的请求（`POST /v1/<op>`，格式见 server/README.md「请求」）
final class Requester: @unchecked Sendable {
    let server: ServerRef
    let identity: Identity
    private let delegate: PinnedDelegate
    private let session: URLSession
    private let lock = NSLock()
    /// 这台设备的钟和服务器差多少秒（服务器说钟不准时照它的时间再发一次）
    private var skew = 0.0

    init(server: ServerRef, identity: Identity) {
        self.server = server
        self.identity = identity
        delegate = PinnedDelegate(pins: server.pins)
        let cfg = URLSessionConfiguration.ephemeral
        cfg.httpCookieStorage = nil
        cfg.urlCache = nil
        cfg.requestCachePolicy = .reloadIgnoringLocalCacheData
        cfg.timeoutIntervalForRequest = 25   // 服务器替 pair_get 最多等 10 秒：给得比它长；手机的网络半死不活时不一直挂着
        cfg.connectionProxyDictionary = [:]   // 和 cca 的 --noproxy 一样：不经系统设置的代理（中转是端到端加密的，代理也看不到）
        cfg.tlsMinimumSupportedProtocolVersion = .TLSv12   // App 为了钉住自签证书关了 ATS（Info.plist）：TLS 的下限照 ATS 的
        session = URLSession(configuration: cfg, delegate: delegate, delegateQueue: nil)
    }

    deinit {
        session.invalidateAndCancel()
    }

    /// 回服务器的 JSON；不是 200 抛 RemoteFailure
    func post(_ op: String, _ extra: [String: Any], again: Bool = true) async throws -> [String: Any] {
        let ts = Date().timeIntervalSince1970 + lock.withLock { skew }
        var obj = extra
        obj["v"] = 1
        obj["op"] = op
        obj["key"] = identity.fp
        obj["ts"] = ts
        obj["nonce"] = random(16).hex
        let body = try JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])
        let sig = identity.sign(Bytes(body))
        guard let url = URL(string: server.url + "/v1/" + op) else { throw RemoteFailure("error") }
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.httpBody = body
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.setValue(Bytes(utf8: sig).b64, forHTTPHeaderField: "X-Cca-Sig")
        let data: Data
        let resp: URLResponse
        do {
            (data, resp) = try await session.data(for: req)
        } catch {
            throw RemoteFailure(delegate.pinFailed ? "pin" : "unreachable")
        }
        let status = (resp as? HTTPURLResponse)?.statusCode ?? 0
        let j = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        if status == 200, let j, j["ok"] as? Bool == true {
            return j
        }
        let err = (j?["error"] as? String).flatMap { e in
            e.range(of: "^[a-z_]{1,32}$", options: .regularExpression) != nil ? e : nil
        } ?? "error"
        if err == "clock", again, let now = jsonNumber(j?["now"]), now.isFinite {
            lock.withLock { skew = now - Date().timeIntervalSince1970 }
            return try await post(op, extra, again: false)
        }
        throw RemoteFailure(err, status: status)
    }
}

// MARK: - 数据端口

/// 一条 TCP 连接（Network.framework）：收到的字节、断开都经回调出来，回调在 queue 上
final class TCPStream: @unchecked Sendable {
    private let conn: NWConnection
    private let queue: DispatchQueue
    var onData: ((Bytes) -> Void)?
    var onReady: (() -> Void)?
    var onClose: (() -> Void)?
    private var closed = false

    init(host: String, port: UInt16, queue: DispatchQueue) {
        self.queue = queue
        let params = NWParameters.tcp
        params.preferNoProxies = true
        conn = NWConnection(host: NWEndpoint.Host(host), port: NWEndpoint.Port(rawValue: port) ?? 1, using: params)
    }

    func start() {
        conn.stateUpdateHandler = { [weak self] st in
            guard let self else { return }
            switch st {
            case .ready:
                self.onReady?()
                self.receive()
            case .failed, .cancelled:
                self.finish()
            case .waiting:
                self.conn.cancel()   // 连不上（没有网、端口不通）：不等系统自己重试，交给上面的退避
            default:
                break
            }
        }
        conn.start(queue: queue)
    }

    private func receive() {
        conn.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [weak self] data, _, done, err in
            guard let self else { return }
            if let data, !data.isEmpty {
                self.onData?(Bytes(data))
            }
            if done || err != nil {
                self.finish()
            } else if !self.closed {
                self.receive()
            }
        }
    }

    func send(_ b: Bytes) {
        guard !closed else { return }
        conn.send(content: Data(b), completion: .contentProcessed { [weak self] err in
            if err != nil { self?.finish() }
        })
    }

    func close() {
        guard !closed else { return }
        closed = true
        conn.cancel()
    }

    private func finish() {
        let first = !closed
        closed = true
        conn.cancel()
        if first || onClose != nil {
            let f = onClose
            onClose = nil
            f?()
        }
    }
}
