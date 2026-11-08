import Foundation

/// 账号在各组里怎么排：用户拖出来的顺序和置顶。只影响 App 里的显示，存在 App 的设置里，不进 `~/.cca`
/// （账号表的顺序、端口、路由器都不受影响）。账号按 `Account.stableID` 认：改别名不会丢。
struct AccountArrangement: Codable, Equatable, Sendable {
    /// 组 → 拖过之后的顺序。没拖过的组没有这一项，照账号表的顺序
    var order: [String: [String]] = [:]
    /// 置顶的账号
    var top: Set<String> = []

    /// 排好的结果：置顶的在前；同一区里排过的按排的顺序，没排过的（新添加的）接在后面，照原来的顺序。
    func arranged<T>(_ items: [T], group: String, id: (T) -> String) -> [T] {
        var rank: [String: Int] = [:]
        for (i, k) in (order[group] ?? []).enumerated() where rank[k] == nil { rank[k] = i }
        return items.enumerated()
            .map { n, item -> (item: T, key: (Int, Int, Int)) in
                let k = id(item)
                return (item, (top.contains(k) ? 0 : 1, rank[k] ?? .max, n))
            }
            .sorted { $0.key < $1.key }
            .map(\.item)
    }

    /// 把 shown 里 offsets 的账号挪到 destination（和 SwiftUI 的 `onMove` 一个意思）。full 是这个组现在的完整顺序，shown 是其中
    /// 正在挪的一段：只换 shown 这几个账号之间的先后，别的账号（另一区的、另一个工具的第三方 API 账号）位置不动。
    mutating func move(group: String, full: [String], shown: [String], from offsets: IndexSet, to destination: Int) {
        let picked = shown.indices.filter { offsets.contains($0) }.map { shown[$0] }
        var moved = shown.indices.filter { !offsets.contains($0) && $0 < destination }.map { shown[$0] }
        moved += picked
        moved += shown.indices.filter { !offsets.contains($0) && $0 >= destination }.map { shown[$0] }
        let members = Set(shown)
        var next = moved.makeIterator()
        order[group] = full.map { members.contains($0) ? next.next() ?? $0 : $0 }
    }

    mutating func setTop(_ id: String, _ on: Bool) {
        if on { top.insert(id) } else { top.remove(id) }
    }
}

/// 账号列表里的一组（侧栏）：账号按用户排的顺序，置顶的在前面。
struct AccountGroup: Identifiable, Equatable, Sendable {
    var department: Employee.Department
    var accounts: [Account]
    var id: Employee.Department { department }
}
