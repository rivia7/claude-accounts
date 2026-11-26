// 两头之间的加密通道（这一头总是接入方）：握手、记录。格式照 server/README.md「远程会话的中转 → 握手、记录」。
import Foundation

public enum ChannelError: Error, Equatable {
    case length, auth, pad, tooLong
}

final class Channel {
    static let version: UInt8 = 1
    static let recordMax = 16384

    private let ka: Mac
    private let pid: Bytes
    private let sid: Bytes
    private let fps: Bytes
    private var nv = Bytes()
    private var sendSeq: UInt64 = 0
    private var recvSeq: UInt64 = 0
    private var out: SealKeys?
    private var inn: SealKeys?

    /// p：配对的密钥；fpH、fpV：配对时记下的宿主（电脑）、自己的指纹
    init?(p: Bytes, pid: String, sid: String, fpH: String, fpV: String) {
        guard let pidB = Bytes.unhex(pid), pidB.count == 16, let sidB = Bytes.unhex(sid), sidB.count == 16 else { return nil }
        ka = Mac(key: hmac(p, Bytes(utf8: "cca-remote/v1/auth")))
        self.pid = pidB
        self.sid = sidB
        fps = Bytes(utf8: fpH + "\0" + fpV + "\0")
    }

    /// 接入方 → 宿主（52 个字节）。nv：测试给定的随机数
    func hello(nv: Bytes? = nil) -> Bytes {
        self.nv = nv ?? random(32)
        return Bytes(utf8: "CR") + [1, Self.version] + self.nv + pid
    }

    /// 宿主回的 68 个字节 → 该发的确认（35 个字节）；宿主的证明不对是 nil（之后什么都不该再发）
    func confirm(_ reply: Bytes) -> Bytes? {
        guard reply.count == 68, reply[0] == 0x43, reply[1] == 0x52, reply[2] == 2, reply[3] == Self.version else { return nil }
        let nh = Bytes(reply[4..<36])
        let tagH = Bytes(reply[36...])
        let ctx = [Self.version] + pid + sid + nv + nh + fps
        guard same(tagH, ka.mac(Bytes(utf8: "host\0"), ctx)) else { return nil }
        let tagV = ka.mac(Bytes(utf8: "viewer\0"), ctx, tagH)
        let ks = Mac(key: ka.mac(Bytes(utf8: "session\0"), ctx, tagH, tagV))
        let key = { (name: String) in Mac(key: ks.mac(Bytes(utf8: name))) }
        out = SealKeys(enc: key("v2h-enc"), mac: key("v2h-mac"))
        inn = SealKeys(enc: key("h2v-enc"), mac: key("h2v-mac"))
        return Bytes(utf8: "CR") + [3] + tagV
    }

    /// 一条记录：序号(8) 密文长度(2) 密文 认证(16)。明文是 长度(2) 内容 零补齐到 32 的整数倍
    func seal(_ data: Bytes) throws -> Bytes {
        guard let out, data.count <= Self.recordMax else { throw ChannelError.tooLong }
        var pt = u16be(data.count) + data
        pt += Bytes(repeating: 0, count: (32 - pt.count % 32) % 32)
        let seq = u64be(sendSeq)
        sendSeq += 1
        xorStream(out.enc, seq, &pt)
        let head = seq + u16be(pt.count)
        return head + pt + out.mac.mac(head, pt).prefix(16)
    }

    /// buf 开头的一条记录 → (内容, 用掉的字节数)；还不够一条是 nil；长度、序号、认证、补齐不对抛错（该断开）
    func open(_ buf: ArraySlice<UInt8>) throws -> (data: Bytes, used: Int)? {
        guard let inn else { throw ChannelError.auth }
        guard buf.count >= 10 else { return nil }
        let s = buf.startIndex
        let n = Int(buf[s + 8]) << 8 | Int(buf[s + 9])
        if n > Self.recordMax + 2 + 31 || n % 32 != 0 || n == 0 { throw ChannelError.length }
        guard buf.count >= 26 + n else { return nil }
        let head = Bytes(buf[s..<s + 10])
        var ct = Bytes(buf[s + 10..<s + 10 + n])
        guard same(head.prefix(8), u64be(recvSeq)), same(buf[s + 10 + n..<s + 26 + n], inn.mac.mac(head, ct).prefix(16)) else {
            throw ChannelError.auth
        }
        recvSeq += 1
        xorStream(inn.enc, Bytes(head.prefix(8)), &ct)
        let size = Int(ct[0]) << 8 | Int(ct[1])
        guard size <= n - 2, ct[(2 + size)...].allSatisfy({ $0 == 0 }) else { throw ChannelError.pad }
        return (Bytes(ct[2..<2 + size]), 26 + n)
    }
}

// MARK: - 帧（记录里装的，和 cca 在一台机器上用的同一套：cli/src/cmd/remote/local.rs）

enum Frame {
    static let hello: UInt8 = 0x01, input: UInt8 = 0x02, resize: UInt8 = 0x03, detach: UInt8 = 0x04
    static let output: UInt8 = 0x12, exit: UInt8 = 0x13, notify: UInt8 = 0x16
    /// 会话现在的大小（列数、行数，和 `size` 一样）：新的宿主只给 HELLO 里说了能缩放的，接上时说一次（在回放前面），变了再说
    static let sessionSize: UInt8 = 0x17
    /// HELLO 的标志：只看；能缩放（会话和这块屏幕不一样大时缩小了画、拖着看：宿主不把这台手机的窗口算进会话的大小，等它打了字
    /// 才照它排，会话多大经 `sessionSize` 告诉它。旧的宿主不认，照旧算进去）
    static let readOnly: UInt8 = 1, zoom: UInt8 = 4
    static let max = 1 << 20

    static func make(_ kind: UInt8, _ payload: Bytes) -> Bytes {
        [kind] + u32be(payload.count) + payload
    }

    static func size(_ cols: Int, _ rows: Int) -> Bytes {
        u16be(cols) + u16be(rows)
    }

    static func validSize(_ cols: Int, _ rows: Int) -> Bool {
        (2...1000).contains(cols) && (2...1000).contains(rows)
    }
}

/// 开场白（中转那一头）：CCA-RELAY/1 view <票据> <cn> <证明>，和服务器该回的那一行
func relayLine(tid: String, tkey: Bytes, cn: String? = nil) -> (line: String, expect: String) {
    let cn = cn ?? random(8).hex
    let k = Mac(key: tkey)
    return ("CCA-RELAY/1 view \(tid) \(cn) \(k.mac(Bytes(utf8: "cview" + tid + cn)).b64url)\n",
            "CCA-RELAY/1 OK \(k.mac(Bytes(utf8: "s" + tid + cn)).b64url)")
}
