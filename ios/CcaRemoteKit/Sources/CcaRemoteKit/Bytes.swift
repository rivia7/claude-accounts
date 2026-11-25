// 字节的小工具：十六进制、base64、大端整数、常量时间比较、随机数、SSH 的长度前缀字符串。
import Foundation
import Security

public typealias Bytes = [UInt8]

extension Array where Element == UInt8 {
    /// 字符串的 UTF-8
    init(utf8 s: String) {
        self = Array(s.utf8)
    }

    var hex: String {
        let digits = Array("0123456789abcdef".utf8)
        var out = [UInt8]()
        out.reserveCapacity(count * 2)
        for b in self {
            out.append(digits[Int(b >> 4)])
            out.append(digits[Int(b & 15)])
        }
        return String(decoding: out, as: UTF8.self)
    }

    /// 小写十六进制 → 字节；不是（大写、单数个、别的字符）是 nil
    static func unhex(_ s: String) -> Bytes? {
        let u = Array(s.utf8)
        guard u.count % 2 == 0 else { return nil }
        func v(_ c: UInt8) -> UInt8? {
            switch c {
            case 0x30...0x39: return c - 0x30
            case 0x61...0x66: return c - 0x61 + 10
            default: return nil
            }
        }
        var out = Bytes()
        out.reserveCapacity(u.count / 2)
        var i = 0
        while i < u.count {
            guard let hi = v(u[i]), let lo = v(u[i + 1]) else { return nil }
            out.append(hi << 4 | lo)
            i += 2
        }
        return out
    }

    /// 标准的 base64（带 =）
    var b64: String {
        Data(self).base64EncodedString()
    }

    /// 网址里用的 base64（-、_，不带 =）
    var b64url: String {
        b64.replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
    }

    /// 标准的 base64 → 字节。只认 A-Z a-z 0-9 + / 和最多两个结尾的 =（和 core.js 的 unb64 一样严）
    static func unb64(_ s: String) -> Bytes? {
        var pad = 0
        for c in s.utf8 {
            switch c {
            case UInt8(ascii: "="): pad += 1
            case UInt8(ascii: "A")...UInt8(ascii: "Z"), UInt8(ascii: "a")...UInt8(ascii: "z"), UInt8(ascii: "0")...UInt8(ascii: "9"),
                 UInt8(ascii: "+"), UInt8(ascii: "/"):
                if pad > 0 { return nil }
            default: return nil
            }
        }
        guard pad <= 2, let d = Data(base64Encoded: s) else { return nil }
        return Bytes(d)
    }
}

/// 8 个字节的大端整数
func u64be(_ n: UInt64) -> Bytes {
    (0..<8).map { UInt8(truncatingIfNeeded: n >> (56 - 8 * UInt64($0))) }
}

func u16be(_ n: Int) -> Bytes {
    [UInt8(truncatingIfNeeded: n >> 8), UInt8(truncatingIfNeeded: n)]
}

func u32be(_ n: Int) -> Bytes {
    [UInt8(truncatingIfNeeded: n >> 24), UInt8(truncatingIfNeeded: n >> 16), UInt8(truncatingIfNeeded: n >> 8), UInt8(truncatingIfNeeded: n)]
}

func readU32(_ b: ArraySlice<UInt8>) -> Int {
    let i = b.startIndex
    return Int(b[i]) << 24 | Int(b[i + 1]) << 16 | Int(b[i + 2]) << 8 | Int(b[i + 3])
}

/// 比两段字节：不因为前几个字节不一样就早回（认证码的比较）
func same<A: Collection, B: Collection>(_ a: A, _ b: B) -> Bool where A.Element == UInt8, B.Element == UInt8 {
    guard a.count == b.count else { return false }
    var d: UInt8 = 0
    for (x, y) in zip(a, b) {
        d |= x ^ y
    }
    return d == 0
}

/// 系统的随机数
func random(_ n: Int) -> Bytes {
    var out = Bytes(repeating: 0, count: n)
    let rc = SecRandomCopyBytes(kSecRandomDefault, n, &out)
    precondition(rc == errSecSuccess, "没有随机数")
    return out
}

/// SSH 的字符串：4 个字节的大端长度，加上内容
func sshString(_ b: Bytes) -> Bytes {
    u32be(b.count) + b
}
