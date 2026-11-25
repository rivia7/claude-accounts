// 这台设备在一台服务器上的身份：一把 Ed25519，请求照 OpenSSH 的 SSHSIG 签名（命名空间 cca-presence）。
import CryptoKit
import Foundation

public struct Identity {
    let key: Curve25519.Signing.PrivateKey
    /// ssh-ed25519 公钥的 blob
    let blob: Bytes
    /// `ssh-ed25519 base64`
    public let pub: String
    /// `SHA256:base64`（不带 =），服务器、电脑那一头认这台设备靠它
    public let fp: String

    /// 32 个字节的私钥（存钥匙串的就是它）；不对是 nil
    public init?(raw: Bytes) {
        guard let k = try? Curve25519.Signing.PrivateKey(rawRepresentation: raw) else { return nil }
        key = k
        blob = sshString(Bytes(utf8: "ssh-ed25519")) + sshString(Bytes(k.publicKey.rawRepresentation))
        pub = "ssh-ed25519 " + blob.b64
        fp = "SHA256:" + sha256(blob).b64.replacingOccurrences(of: "=", with: "")
    }

    /// 新的一把：32 个字节的私钥
    public static func generate() -> Bytes {
        Bytes(Curve25519.Signing.PrivateKey().rawRepresentation)
    }

    public var raw: Bytes {
        Bytes(key.rawRepresentation)
    }

    /// ssh-keygen -Y sign 那种签名（OpenSSH 的 PROTOCOL.sshsig），PEM 那样的文本
    func sign(_ data: Bytes) -> String {
        let ns = "cca-presence"
        let digest = Bytes(SHA512.hash(data: data))
        let rest = sshString(Bytes(utf8: ns)) + sshString([]) + sshString(Bytes(utf8: "sha512"))
        let sig = Bytes((try? key.signature(for: Bytes(utf8: "SSHSIG") + rest + sshString(digest))) ?? Data())
        let out = Bytes(utf8: "SSHSIG") + [0, 0, 0, 1] + sshString(blob) + rest
            + sshString(sshString(Bytes(utf8: "ssh-ed25519")) + sshString(sig))
        let b = Array(out.b64)
        let lines = stride(from: 0, to: b.count, by: 70).map { String(b[$0..<Swift.min($0 + 70, b.count)]) }
        return "-----BEGIN SSH SIGNATURE-----\n" + lines.joined(separator: "\n") + "\n-----END SSH SIGNATURE-----\n"
    }
}
