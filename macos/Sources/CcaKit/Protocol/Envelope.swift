import Foundation

/// 这个 App 支持的 `cca _gui hello` 的 `api`（PROTOCOL.md：GUI_API）。
enum GUIAPI {
    static let supported: ClosedRange<Int> = 1...1

    static func isSupported(_ api: Int?) -> Bool {
        guard let api else { return false }
        return supported.contains(api)
    }
}

/// 每份文档都带的 `version` 和 `kind`。
struct Envelope: Decodable, Sendable {
    var version: Int?
    var kind: String?

    enum CodingKeys: String, CodingKey {
        case version, kind
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
    }
}

enum DocumentError: Error, CustomStringConvertible, Equatable {
    case empty
    case notJSON(String)
    case unexpectedKind(expected: String, got: String)

    var description: String {
        switch self {
        case .empty: return "cca 没有输出 JSON"
        case .notJSON(let why): return "cca 的输出不是 JSON：\(why)"
        case .unexpectedKind(let e, let g): return "cca 的输出不对：应该是 \(e)，实际是 \(g)"
        }
    }
}

enum DocumentDecoder {
    /// 解码 cca 写到 stdout 的一份 JSON。stdout 应该只有这一份 JSON；万一前面混进了别的行，取最后一个以 `{` 开头的行再试。
    static func decode<T: CcaDocument>(_ type: T.Type, from data: Data) throws -> T {
        let trimmed = trim(data)
        guard !trimmed.isEmpty else { throw DocumentError.empty }
        do {
            return try decodeChecked(T.self, trimmed)
        } catch let e as DocumentError {
            throw e
        } catch {
            if let line = lastJSONLine(trimmed), line != trimmed, let v = try? decodeChecked(T.self, line) {
                return v
            }
            throw DocumentError.notJSON(String(describing: error))
        }
    }

    private static func decodeChecked<T: CcaDocument>(_ type: T.Type, _ data: Data) throws -> T {
        let decoder = JSONDecoder()
        let env = try decoder.decode(Envelope.self, from: data)
        if let want = T.expectedKind, let got = env.kind, got != want {
            throw DocumentError.unexpectedKind(expected: want, got: got)
        }
        return try decoder.decode(T.self, from: data)
    }

    static func trim(_ data: Data) -> Data {
        var s = data.startIndex, e = data.endIndex
        func ws(_ b: UInt8) -> Bool { b == 0x20 || b == 0x0A || b == 0x0D || b == 0x09 }
        while s < e, ws(data[s]) { s = data.index(after: s) }
        while e > s, ws(data[data.index(before: e)]) { e = data.index(before: e) }
        return data.subdata(in: s..<e)
    }

    private static func lastJSONLine(_ data: Data) -> Data? {
        let lines = data.split(separator: 0x0A, omittingEmptySubsequences: true)
        for line in lines.reversed() {
            let t = trim(Data(line))
            if t.first == UInt8(ascii: "{") { return t }
        }
        return nil
    }
}
