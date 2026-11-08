import Foundation
import CryptoKit

/// ChatGPT 官方软件包的 Ed25519 验签。公钥固定在已安装的 cca 中，不接受命令行或环境覆盖。
enum ClientUpdateSignature {
    static let option = "--verify-client-update"
    private static let publicKey = "mNfr1v9t63BfgDtlw4C8lRvSY6uMggIXABDOCi3tS6k="

    static func verify(archive: URL, signature: String) -> Bool {
        guard let keyData = Data(base64Encoded: publicKey), keyData.count == 32,
              let signatureData = Data(base64Encoded: signature), signatureData.count == 64,
              let key = try? Curve25519.Signing.PublicKey(rawRepresentation: keyData),
              let archiveData = try? Data(contentsOf: archive, options: .mappedIfSafe) else { return false }
        return key.isValidSignature(signatureData, for: archiveData)
    }

    /// 不进入 NSApplication；0 有效，1 校验或读取失败，2 参数不合法。
    static func run(arguments: [String]) -> Int32 {
        guard arguments.count == 3, arguments[0] == option, arguments[1].hasPrefix("/") else { return 2 }
        return verify(archive: URL(fileURLWithPath: arguments[1]), signature: arguments[2]) ? 0 : 1
    }
}
