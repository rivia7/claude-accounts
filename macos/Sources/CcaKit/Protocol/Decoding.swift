import Foundation

// 容错解码：cca 只会“加”字段、枚举值和事件类型（PROTOCOL.md 开头的兼容规则），所以这里
// - 缺字段、null、类型不对一律当作没有（不抛错，不让一个字段毁掉整份文档）；
// - 枚举按字符串收（OpenEnum），不认识的值照样收下，只是不等于任何已知常量；
// - 数字可能是整数也可能是浮点数（也容忍写成字符串的数字）；
// - 数组里坏掉的元素跳过，不影响其它元素；
// - 时间只用 `*_epoch`（Unix 秒）；旧字段 `resets_at` / `fetched_at` / `generated_at` 不解析：`*_epoch` 为 null 就是 cca 自己也解析不了，不显示。
// 不用 .convertFromSnakeCase：它连字典的键都改写。每个模型都写明 CodingKeys。

/// 字符串枚举：不认识的值也能解码（当“其它”），永远不会让解码失败。
protocol OpenEnum: RawRepresentable, Codable, Hashable, Sendable, CustomStringConvertible where RawValue == String {
    init(rawValue: String)
}

extension OpenEnum {
    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if let s = try? c.decode(String.self) {
            self.init(rawValue: s)
        } else if let i = try? c.decode(Int.self) {
            self.init(rawValue: String(i))
        } else if let b = try? c.decode(Bool.self) {
            self.init(rawValue: b ? "true" : "false")
        } else {
            self.init(rawValue: "")
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encode(rawValue)
    }

    var description: String { rawValue }
}

/// 任意键（读字典、注入未知键的测试用）。
struct AnyKey: CodingKey, Hashable {
    var stringValue: String
    var intValue: Int?
    init(_ s: String) { stringValue = s; intValue = nil }
    init?(stringValue: String) { self.stringValue = stringValue; intValue = nil }
    init?(intValue: Int) { stringValue = String(intValue); self.intValue = intValue }
}

/// 什么都收、什么都不读：用来跳过数组里坏掉的元素。
struct SkipValue: Decodable {
    init(from decoder: Decoder) throws {}
}

extension KeyedDecodingContainer {
    /// 缺、null、类型不对都返回 nil。
    func lossy<T: Decodable>(_ type: T.Type, _ key: Key) -> T? {
        guard contains(key) else { return nil }
        if (try? decodeNil(forKey: key)) == true { return nil }
        return try? decode(T.self, forKey: key)
    }

    func string(_ key: Key) -> String? {
        if let s = lossy(String.self, key) { return s }
        return nil
    }

    /// 字符串；数字也收（例如 credits 的 balance 可能写成数字）。
    func stringish(_ key: Key) -> String? {
        if let s = lossy(String.self, key) { return s }
        if let i = lossy(Int.self, key) { return String(i) }
        if let d = lossy(Double.self, key) { return String(d) }
        return nil
    }

    func bool(_ key: Key) -> Bool? {
        lossy(Bool.self, key)
    }

    /// 整数或浮点数（或写成字符串的数字）。
    func double(_ key: Key) -> Double? {
        if let d = lossy(Double.self, key), d.isFinite { return d }
        if let s = lossy(String.self, key), let d = Double(s), d.isFinite { return d }
        return nil
    }

    /// 整数；7901.0 这样的浮点数也收，带小数的不收。
    func int(_ key: Key) -> Int? {
        if let i = lossy(Int.self, key) { return i }
        if let d = double(key), d == d.rounded(), abs(d) < 9.0e15 { return Int(d) }
        return nil
    }

    /// 数组：坏掉的元素跳过；缺或不是数组返回 nil。
    func lossyArray<T: Decodable>(_ type: T.Type, _ key: Key) -> [T]? {
        guard contains(key), var c = try? nestedUnkeyedContainer(forKey: key) else { return nil }
        var out: [T] = []
        while !c.isAtEnd {
            let before = c.currentIndex
            if (try? c.decodeNil()) == true { continue }
            if let v = try? c.decode(T.self) {
                out.append(v)
            } else {
                _ = try? c.decode(SkipValue.self)
            }
            if c.currentIndex == before { break }   // 防止解码器卡住
        }
        return out
    }

    /// 字典：值坏掉的键跳过；缺或不是对象返回 nil。
    func lossyDictionary<T: Decodable>(_ type: T.Type, _ key: Key) -> [String: T]? {
        guard contains(key), let c = try? nestedContainer(keyedBy: AnyKey.self, forKey: key) else { return nil }
        var out: [String: T] = [:]
        for k in c.allKeys {
            if let v = c.lossy(T.self, k) { out[k.stringValue] = v }
        }
        return out
    }
}

/// 一份 cca 输出的 JSON 文档。
protocol CcaDocument: Decodable, Sendable {
    /// `kind` 应该是什么；nil 表示不检查（例如旧版 usage --json 没有 kind）。
    static var expectedKind: String? { get }
}
