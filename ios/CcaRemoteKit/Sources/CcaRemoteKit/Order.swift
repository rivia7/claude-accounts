/// 会话列表里会话的先后（iOS App 的「远程会话」页拖动排的）：会话编号排成一列，App 记在 UserDefaults 里。各台电脑的会话各排各的：
/// 记着的照记着的先后，没记着的（新起的会话）照原来的先后接在后面。会话结束了编号不会再出现：刚排过的那一组写在最前面，最多记
/// limit 个，多了的从最后去掉（早就结束了的会话）。和 Mac App 的 RemoteOrder 是同一个规矩
public struct SessionOrder: Equatable, Sendable {
    /// 最多记几个会话编号（一台电脑最多 100 个会话，几台加起来也够）
    public static let limit = 400
    public private(set) var ids: [String]

    /// 记着的那一列（UserDefaults 里读回来的）：重复的、空的去掉，最多 limit 个
    public init(ids: [String] = []) {
        var seen = Set<String>()
        self.ids = Array(ids.filter { !$0.isEmpty && seen.insert($0).inserted }.prefix(Self.limit))
    }

    /// 照记着的先后排：记着的在前、照记着的先后，没记着的接在后面、照原来的先后
    public func sorted<T>(_ items: [T], id: (T) -> String) -> [T] {
        var rank: [String: Int] = [:]
        for (i, k) in ids.enumerated() where rank[k] == nil { rank[k] = i }
        return items.enumerated()
            .map { n, item in (item, rank[id(item)] ?? .max, n) }
            .sorted { ($0.1, $0.2) < ($1.1, $1.2) }
            .map(\.0)
    }

    /// 一台电脑的会话排好的先后（拖完以后的整组）：写在最前面，别的照旧
    public mutating func record(_ group: [String]) {
        let mine = Set(group)
        ids = Array((group + ids.filter { !mine.contains($0) }).prefix(Self.limit))
    }
}
