// 配对、会话的说明用到的密码学。格式照 server/README.md「远程会话的中转」，和 cca 的辅助程序（cli/src/cmd/remote/crypto.rs）、
// tests/kit/spec.rs 是同一套：几份实现要能互相配对、握手、收发。只用 HMAC-SHA256（CryptoKit）和 PBKDF2（CommonCrypto）。
import CommonCrypto
import CryptoKit
import Foundation

/// 一把 HMAC-SHA256 的密钥：初始化好的状态留着，每次复制一份接着算（加密记录要一块块地出密钥流）
public struct Mac {
    private let base: HMAC<SHA256>

    init(key: Bytes) {
        base = HMAC<SHA256>(key: SymmetricKey(data: key))
    }

    func mac(_ parts: Bytes...) -> Bytes {
        var h = base
        for p in parts {
            h.update(data: p)
        }
        return Bytes(h.finalize())
    }
}

func hmac(_ key: Bytes, _ msg: Bytes) -> Bytes {
    Mac(key: key).mac(msg)
}

func sha256(_ b: Bytes) -> Bytes {
    Bytes(SHA256.hash(data: b))
}

/// 加密和认证各一把
struct SealKeys {
    let enc: Mac
    let mac: Mac

    init(enc: Mac, mac: Mac) {
        self.enc = enc
        self.mac = mac
    }

    /// 从一把密钥派生：HMAC(k, "enc")、HMAC(k, "mac")
    init(from k: Mac) {
        enc = Mac(key: k.mac(Bytes(utf8: "enc")))
        mac = Mac(key: k.mac(Bytes(utf8: "mac")))
    }
}

/// 密钥流：HMAC(enc, nonce + 8 字节大端的块号)，一块 32 个字节，和 data 异或（原地改）
func xorStream(_ enc: Mac, _ nonce: Bytes, _ data: inout Bytes) {
    var at = 0
    var i: UInt64 = 0
    while at < data.count {
        let ks = enc.mac(nonce, u64be(i))
        let n = Swift.min(32, data.count - at)
        for j in 0..<n {
            data[at + j] ^= ks[j]
        }
        at += 32
        i += 1
    }
}

// MARK: - 封好的一份（{n, c, t}：配对时交换的、会话的说明）

private func sealTag(_ label: String, _ mac: Mac, _ ad: [String], _ n: Bytes, _ c: Bytes) -> Bytes {
    mac.mac(Bytes(utf8: ([label] + ad).map { $0 + "\0" }.joined()), n, c)
}

func seal(_ label: String, _ keys: SealKeys, _ ad: [String], _ pt: Bytes, nonce: Bytes? = nil) -> [String: String] {
    let n = nonce ?? random(16)
    var c = pt
    xorStream(keys.enc, n, &c)
    return ["n": n.b64, "c": c.b64, "t": sealTag(label, keys.mac, ad, n, c).b64]
}

/// 认证过的明文；认证不过、样子不对是 nil
func unseal(_ label: String, _ keys: SealKeys, _ ad: [String], _ blob: Any?) -> Bytes? {
    guard let blob = blob as? [String: Any], let ns = blob["n"] as? String, let cs = blob["c"] as? String, let ts = blob["t"] as? String,
          let n = Bytes.unb64(ns), var c = Bytes.unb64(cs), let t = Bytes.unb64(ts), n.count == 16,
          same(t, sealTag(label, keys.mac, ad, n, c))
    else { return nil }
    xorStream(keys.enc, n, &c)
    return c
}

/// JSON，用空格补齐到 256 的整数倍（密文的长短不透露名字有多长）
func padded(_ obj: [String: Any]) -> Bytes {
    var pt = Bytes((try? JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])) ?? Data("{}".utf8))
    pt += Bytes(repeating: 0x20, count: (256 - pt.count % 256) % 256)
    return pt
}

func parsed(_ pt: Bytes) -> [String: Any]? {
    (try? JSONSerialization.jsonObject(with: Data(pt))) as? [String: Any]
}

/// JSON 里的一个数（true / false 不算：JSONSerialization 把它们也给成 NSNumber）
func jsonNumber(_ x: Any?) -> Double? {
    guard let n = x as? NSNumber, CFGetTypeID(n) != CFBooleanGetTypeID() else { return nil }
    return n.doubleValue
}

/// JSON 里的一个整数
func jsonInt(_ x: Any?) -> Int? {
    guard let d = jsonNumber(x), d.rounded() == d, abs(d) < 1e15 else { return nil }
    return Int(d)
}

// MARK: - 配对码

private let b32 = Array("0123456789ABCDEFGHJKMNPQRSTVWXYZ".utf8)   // Crockford base32

/// 输入的配对码（大小写、横线、空格随意；I L 当 1、O 当 0）→ 10 个字节；不对是 nil
public func codeBytes(_ input: String) -> Bytes? {
    var digits = [UInt8]()
    for ch in input.uppercased().unicodeScalars {
        switch ch {
        case "-", " ", "\t", "\n", "\r": continue
        case "I", "L": digits.append(1)
        case "O": digits.append(0)
        default:
            guard ch.isASCII, let v = b32.firstIndex(of: UInt8(ch.value)) else { return nil }
            digits.append(UInt8(v))
        }
    }
    guard digits.count == 16 else { return nil }
    var out = Bytes()
    var acc: UInt32 = 0
    var bits = 0
    for d in digits {
        acc = acc << 5 | UInt32(d)
        bits += 5
        if bits >= 8 {
            bits -= 8
            out.append(UInt8(truncatingIfNeeded: acc >> UInt32(bits)))
            acc &= (1 << UInt32(bits)) - 1
        }
    }
    return out
}

/// 10 个字节 → XXXX-XXXX-XXXX-XXXX
public func codeText(_ c: Bytes) -> String {
    var out = [UInt8]()
    var acc: UInt32 = 0
    var bits = 0
    for b in c {
        acc = acc << 8 | UInt32(b)
        bits += 8
        while bits >= 5 {
            bits -= 5
            out.append(b32[Int(acc >> UInt32(bits) & 31)])
            acc &= (1 << UInt32(bits)) - 1
        }
    }
    var s = ""
    for (i, ch) in out.enumerated() {
        if i > 0, i % 4 == 0 { s += "-" }
        s += String(UnicodeScalar(ch))
    }
    return s
}

// MARK: - 配对

struct PairKeys {
    let kc: Mac
    let keys: SealKeys
    let rid: String
}

/// PBKDF2-HMAC-SHA256
func pbkdf2(_ password: Bytes, salt: Bytes, rounds: UInt32, length: Int) -> Bytes {
    var out = Bytes(repeating: 0, count: length)
    let rc = password.withUnsafeBufferPointer { pw in
        CCKeyDerivationPBKDF(CCPBKDFAlgorithm(kCCPBKDF2), pw.baseAddress.map { UnsafeRawPointer($0).assumingMemoryBound(to: CChar.self) },
                             pw.count, salt, salt.count, CCPseudoRandomAlgorithm(kCCPRFHmacAlgSHA256), rounds, &out, length)
    }
    precondition(rc == kCCSuccess, "PBKDF2")
    return out
}

/// 配对码 → {kc, keys, rid}。配对码只有 80 位，所以这一步故意算得慢（PBKDF2 65536 轮）：别在主线程上调
func pairKeys(_ code: Bytes) -> PairKeys {
    let kc = Mac(key: pbkdf2(code, salt: Bytes(utf8: "cca-remote/v1/pair"), rounds: 65536, length: 32))
    return PairKeys(kc: kc, keys: SealKeys(from: kc), rid: String(kc.mac(Bytes(utf8: "rid")).hex.prefix(32)))
}

/// 配对时放到服务器上的那一份。role："a" 显示配对码的 / "b" 输入的
func pairMsg(_ keys: SealKeys, rid: String, role: String, fp: String, nonce: Bytes, name: String) -> [String: String] {
    seal("cca-remote-pair", keys, [rid, role], padded(["v": 1, "fp": fp, "n": nonce.hex, "name": name]))
}

struct PairOpened {
    let fp: String
    let n: Bytes
    let name: String
}

func pairOpen(_ keys: SealKeys, rid: String, role: String, _ blob: Any?) -> PairOpened? {
    guard let pt = unseal("cca-remote-pair", keys, [rid, role], blob), let j = parsed(pt), jsonInt(j["v"]) == 1, let fp = j["fp"] as? String, let ns = j["n"] as? String, let n = Bytes.unhex(ns), n.count == 32
    else { return nil }
    return PairOpened(fp: fp, n: n, name: j["name"] as? String ?? "")
}

/// 两台设备之间的密钥 P 和它的编号（服务器只知道编号）。a 是显示配对码的那台
func pairSecret(_ kc: Mac, na: Bytes, nb: Bytes, fpa: String, fpb: String) -> (p: Bytes, pid: String) {
    let p = kc.mac(Bytes(utf8: "secret\0"), na, nb, Bytes(utf8: fpa + "\0" + fpb + "\0"))
    return (p, String(hmac(p, Bytes(utf8: "cca-remote/v1/pid")).hex.prefix(32)))
}

/// 对方写的、服务器转交的文字，显示之前：去掉控制字符、双向控制字符，限制长度（按 Unicode 码位数），去掉两头的空白
public func clean(_ s: Any?, _ max: Int) -> String {
    guard let s = s as? String else { return "" }
    let kept = s.unicodeScalars.filter { u in
        let v = u.value
        return !(v <= 0x1F || (0x7F...0x9F).contains(v) || v == 0x200E || v == 0x200F || (0x202A...0x202E).contains(v)
            || (0x2066...0x2069).contains(v))
    }
    var out = String.UnicodeScalarView()
    out.append(contentsOf: kept.prefix(max))
    return String(out).trimmingCharacters(in: .whitespacesAndNewlines)
}

/// 会话的说明（宿主封给这个配对的）
public struct SessionInfo: Sendable, Equatable {
    public let machine: String
    public let tool: String
    public let name: String
    public let cwd: String
    public let since: Double
    /// 只看（宿主那边定的）
    public let viewOnly: Bool
    /// 会话用的是哪个账号（账号摘要，和账号的状态里的 AccountStatus.aid 一样：「账号」页照它把会话放到账号下面）；
    /// 那台电脑不在 Team 里、旧版的宿主没给是空的
    public let aid: String

    public init(machine: String, tool: String, name: String, cwd: String, since: Double, viewOnly: Bool, aid: String = "") {
        self.machine = machine
        self.tool = tool
        self.name = name
        self.cwd = cwd
        self.since = since
        self.viewOnly = viewOnly
        self.aid = aid
    }
}

/// P 派生的“会话的说明”的密钥
func dirKeys(_ p: Bytes) -> SealKeys {
    SealKeys(from: Mac(key: hmac(p, Bytes(utf8: "cca-remote/v1/dir"))))
}

func dirOpen(_ keys: SealKeys, pid: String, sid: String, _ blob: Any?) -> SessionInfo? {
    guard let pt = unseal("cca-remote-dir", keys, [pid, sid], blob), let j = parsed(pt), jsonInt(j["v"]) == 1 else { return nil }
    let since = jsonNumber(j["since"]).flatMap { $0.isFinite ? $0 : nil } ?? 0
    let aid = (j["aid"] as? String).flatMap { lowerHex($0, 32) ? $0 : nil } ?? ""
    return SessionInfo(machine: clean(j["machine"], 48), tool: clean(j["tool"], 16), name: clean(j["name"], 48), cwd: clean(j["cwd"], 200),
                       since: since, viewOnly: j["access"] as? String == "view", aid: aid)
}
