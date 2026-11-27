import Foundation

/// 「远程会话」页里会话的先后（拖动排的）：会话编号排成一列，记在 App 的设置里（`AppModel.Prefs.remoteOrder`，JSON 数组）。
/// 排的时候各台机器的会话各排各的：记着的照记着的先后，没记着的（新起的会话）照原来的先后接在后面。会话结束了编号不会再出现：
/// 刚排过的那一组写在最前面，最多记 `limit` 个，多了的从最后去掉（早就结束了的会话）。
struct RemoteOrder: Equatable, Sendable {
    /// 最多记几个会话编号（一台机器最多 100 个会话，几台加起来也够）
    static let limit = 400
    private(set) var ids: [String] = []

    init() {}

    /// 从设置里读回来（`stored` 写的那一份）；读不出来是空的
    init(stored: String?) {
        guard let s = stored, let list = try? JSONDecoder().decode([String].self, from: Data(s.utf8)) else { return }
        ids = Array(list.prefix(Self.limit))
    }

    /// 写进设置的那一份
    var stored: String {
        let d = (try? JSONEncoder().encode(ids)) ?? Data("[]".utf8)
        return String(decoding: d, as: UTF8.self)
    }

    /// 照记着的先后排：记着的在前、照记着的先后，没记着的接在后面、照原来的先后
    func sorted<T>(_ items: [T], id: (T) -> String) -> [T] {
        var rank: [String: Int] = [:]
        for (i, k) in ids.enumerated() where rank[k] == nil { rank[k] = i }
        return items.enumerated()
            .map { n, item in (item, rank[id(item)] ?? .max, n) }
            .sorted { ($0.1, $0.2) < ($1.1, $1.2) }
            .map(\.0)
    }

    /// 一台机器的会话排好的先后（拖完以后的整组）：写在最前面，别的照旧
    mutating func record(_ group: [String]) {
        let mine = Set(group)
        ids = Array((group + ids.filter { !mine.contains($0) }).prefix(Self.limit))
    }
}
