// Mac 上「和手机配对…」那个二维码里的链接（cca remote pair --web 给的）：
// cca-remote://pair?v=1&u=服务器的地址&p=证书公钥的指纹（0～4 个）&c=配对码&n=机器名[&t=Team 的名字]
// 格式见 docs/dev/ios.md「二维码里的链接」，出它的是 cli/src/cmd/remote/pair.rs 的 app_link。
import Foundation

/// 一台中转服务器：地址（https://主机[:端口]，结尾不带 /）和它的证书公钥的指纹（curl 的 --pinnedpubkey 那种写法，
/// sha256//base64）。没有指纹时照系统的证书机构验
public struct ServerRef: Codable, Hashable, Sendable {
    public let url: String
    public let pins: [String]

    public init?(url: String, pins: [String]) {
        guard let u = URLComponents(string: url), u.scheme == "https", let host = u.host, !host.isEmpty, u.user == nil, u.password == nil,
              u.path.isEmpty || u.path == "/", u.query == nil, u.fragment == nil, pins.count <= 4, pins.allSatisfy(pinOK)
        else { return nil }
        self.url = url.hasSuffix("/") ? String(url.dropLast()) : url
        self.pins = pins
    }

    /// 主机名（IPv6 不带方括号）
    public var host: String {
        URLComponents(string: url)?.host ?? ""
    }
}

func pinOK(_ p: String) -> Bool {
    guard p.hasPrefix("sha256//") else { return false }
    let b = String(p.dropFirst(8))
    return b.count == 44 && Bytes.unb64(b)?.count == 32
}

public struct PairInvite: Equatable, Sendable {
    public let server: ServerRef
    /// 10 个字节的配对码
    public let code: Bytes
    /// 显示配对码的那台机器自己说的名字（只是给人看的）
    public let machine: String
    /// 配对记在哪个 Team 的服务器上（Team 的名字，给人看的；维护者的服务器没有）
    public let team: String?

    /// 扫到的、粘贴的文字 → 配对的邀请；不是 cca 的链接、样子不对是 nil
    public init?(_ text: String) {
        let s = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard s.count <= 2048, let c = URLComponents(string: s), c.scheme == "cca-remote", c.host == "pair",
              c.path.isEmpty, let items = c.percentEncodedQueryItems
        else { return nil }
        var one = [String: String]()
        var pins = [String]()
        for it in items {
            guard let v = it.value?.removingPercentEncoding else { return nil }
            if it.name == "p" {
                pins.append(v)
            } else if one[it.name] == nil {
                one[it.name] = v
            } else {
                return nil   // 同一项写了两次：不猜是哪一个
            }
        }
        guard one["v"] == "1", let u = one["u"], let server = ServerRef(url: u, pins: pins), let cs = one["c"], let code = codeBytes(cs)
        else { return nil }
        self.server = server
        self.code = code
        machine = clean(one["n"], 48)
        team = one["t"].map { clean($0, 64) }.flatMap { $0.isEmpty ? nil : $0 }
    }
}
