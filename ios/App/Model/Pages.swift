import CcaRemoteKit
import Foundation

/// 一台电脑放上来的一个账号（用量统计、折合金额页的一行）：哪台电脑、它放上来的那一份、账号
struct HostedAccount: Identifiable {
    let peer: Peer
    let host: HostStatus
    let account: AccountStatus
    /// 几台电脑上都有它（同一个 Team 里的电脑给的账号摘要一样），只列了这一份
    var shared = false

    var ref: AccountRef { AccountRef(pid: peer.pid, id: account.id) }
    var id: AccountRef { ref }
}

/// 用量统计页的一行（Mac App 的 StatsLine）：一个账号；Antigravity 的账号是它的一组额度（电脑上的 cca 每组各记一份每周用量），
/// 还没有记录时也是一行
struct StatsLine: Identifiable {
    let row: HostedAccount
    /// Antigravity 的一组；别的工具、还没有记录的 Antigravity 账号是 nil
    let group: GroupWeeks?

    var weeks: UsageWeeks? { group?.weeks ?? row.account.weeks }
    var id: String { row.peer.pid + "/" + row.account.id + "#" + (group?.id ?? "") }
}

/// 一组：一个工具的账号（Mac App 的 AccountGroup：Claude、ChatGPT、Antigravity 各一组）
struct HostedGroup: Identifiable {
    let tool: String
    let rows: [HostedAccount]
    var id: String { tool }
}

extension AppModel {
    /// 各台电脑放上来的账号：照配对的先后、各台电脑给的先后（取消了配对、不让看的电脑没有）
    func hostedAccounts() -> [HostedAccount] {
        peers.flatMap { p -> [HostedAccount] in
            guard let l = listing(p), !l.gone, let st = l.status, !st.off else { return [] }
            return st.accounts.map { HostedAccount(peer: p, host: st, account: $0) }
        }
    }
}

/// 用量统计、折合金额页列哪些账号、怎么分组（纯函数）
enum Pages {
    /// 几台电脑上的同一个账号只留一份（会话说明里的账号摘要 aid 一样：同一个 Team 里的电脑算的；没有摘要的认不出，照留）：
    /// 留 better 认为更好的那份（一样好时留放上来得晚的），位置照头一次出现的
    static func distinct(_ all: [HostedAccount], better: (HostedAccount) -> Bool = { _ in true }) -> [HostedAccount] {
        var out: [HostedAccount] = []
        var at: [String: Int] = [:]
        for r in all {
            guard !r.account.aid.isEmpty else {
                out.append(r)
                continue
            }
            let key = r.account.tool + "\u{0}" + r.account.aid
            guard let i = at[key] else {
                at[key] = out.count
                out.append(r)
                continue
            }
            let old = out[i]
            let (a, b) = (better(r), better(old))
            var keep = a != b ? (a ? r : old) : (r.host.putAt > old.host.putAt ? r : old)
            keep.shared = true
            out[i] = keep
        }
        return out
    }

    /// 按工具分组：Claude、ChatGPT、Antigravity 的先后（cca 的 TOOLS），认不出的新工具照出现的先后排在后面；没有账号的组不列
    static func groups(_ rows: [HostedAccount]) -> [HostedGroup] {
        var tools = AccountShelf.knownTools.filter { t in rows.contains { $0.account.tool == t } }
        for r in rows where !tools.contains(r.account.tool) { tools.append(r.account.tool) }
        return tools.map { t in HostedGroup(tool: t, rows: rows.filter { $0.account.tool == t }) }
    }

    /// 用量统计页的账号：官方账号（第三方 API 没有订阅的额度），Antigravity 的也在（每组额度一行：statsLines）
    static func statsRows(_ all: [HostedAccount]) -> [HostedAccount] {
        distinct(all.filter { !$0.account.isAPI }) { $0.account.weeks != nil || $0.account.weeksGroups != nil }
    }

    /// 一个账号在用量统计页上的几行：Antigravity 有按组记的每周用量时每组一行，别的一个账号一行
    static func statsLines(_ r: HostedAccount) -> [StatsLine] {
        guard let groups = r.account.weeksGroups, !groups.isEmpty else { return [StatsLine(row: r, group: nil)] }
        return groups.map { StatsLine(row: r, group: $0) }
    }

    /// 折合金额页的账号：官方账号，那台电脑上的 cca 开着折合金额（放上来了折合金额那几样，或者账号太多没带）
    static func costRows(_ all: [HostedAccount]) -> [HostedAccount] {
        distinct(all.filter { !$0.account.isAPI && ($0.host.cost != nil || $0.host.costLeftOut) }) { $0.account.cost != nil }
    }

    /// 列出来的账号里有几台电脑：两台以上时每一行写上是哪台电脑的（几台电脑上都有的那几个不写）
    static func machines(_ rows: [HostedAccount]) -> Int {
        Set(rows.map(\.peer.pid)).count
    }
}
