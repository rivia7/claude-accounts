import Foundation

/// 账号在各组里怎么排。顺序以账号表为准（拖动、置顶之后 App 写回账号表，`cca reorder`，终端里的菜单和编号也是这个顺序）；
/// 这里只记置顶（存在 App 的设置里），和还没写进账号表的顺序。账号按 `Account.stableID` 认：改别名不会丢。
struct AccountArrangement: Codable, Equatable, Sendable {
    /// 组 → 刚拖出来、还没写进账号表的顺序（写好了就清掉；旧版 cca 不支持写回时一直用它）
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
