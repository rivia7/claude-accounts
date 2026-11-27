import CcaRemoteKit
import Foundation

/// 「账号」页推进去的一个账号：哪台电脑（配对编号）上的哪个账号（这个配对里认这个账号的编号，AccountStatus.id）
struct AccountRef: Hashable {
    let pid: String
    let id: String
}

/// 挡住一个账号的问题（Mac App 的 Employee.Mood 里放上来的状态看得出的那几个；端口、出口的问题手机上不知道）
enum AccountTrouble: Equatable {
    case loggedOut
    case wrongAccount
    case expired
    /// 5 小时或本周的额度用完了：until 是用完的窗口里最晚重置的时间
    case usedUp(until: Double?)

    /// 要人动手的（额度用完只是等）
    var needsAttention: Bool {
        if case .usedUp = self { return false }
        return true
    }
}

/// 一个账号现在怎么样，照 Mac App 的总览（Employee、Diagnosis）：在用时头条总是在用，有问题时问题跟在后面；没在用时是那个问题，
/// 没有问题是空闲。快用完了是小标记；建议先用（本周额度快重置了、还剩不少）只在没有问题、额度不紧的时候
struct AccountMood: Equatable {
    let busy: Bool
    let trouble: AccountTrouble?
    let nearLimit: Bool
    let useFirst: UseFirst?

    init(_ a: AccountStatus, now: Double) {
        busy = a.busy
        if a.login == "logged_out" {
            trouble = .loggedOut
        } else if a.login == "email_mismatch" {
            trouble = .wrongAccount
        } else if a.usage?.errorCode == "auth" {
            trouble = .expired
        } else if let b = a.blocking(now: now) {
            trouble = .usedUp(until: b.resetsEpoch)
        } else {
            trouble = nil
        }
        nearLimit = trouble == nil && a.nearLimit(now: now)
        let f = a.usage?.useFirst.flatMap { $0.isActive(now: now) ? $0 : nil }
        useFirst = trouble == nil && !nearLimit && !a.fiveHourPressed(now: now) ? f : nil
    }
}

/// 点名（「账号」页顶上那一行，Mac App 的 OfficeCensus）：几个账号，几个在用、几个空闲、几个额度用完……
struct AccountCensus: Equatable {
    var total = 0, working = 0, idle = 0, resting = 0, absent = 0, wrongPerson = 0, expired = 0

    init(_ all: [AccountStatus], now: Double) {
        total = all.count
        for a in all {
            let m = AccountMood(a, now: now)
            if m.busy {
                working += 1
                continue
            }
            switch m.trouble {
            case nil: idle += 1
            case .usedUp: resting += 1
            case .loggedOut: absent += 1
            case .wrongAccount: wrongPerson += 1
            case .expired: expired += 1
            }
        }
    }
}

/// 「账号」页上面的工具切换和按套餐档位筛（照 Mac App 侧栏、菜单栏面板的工具切换和 Team 筛选）：只看一个工具的账号，可以只看一档套餐。
/// 只管显示；选的记在 AppModel（accountTool、accountTier）。记着的工具、档位已经没有账号了时照默认的看，记着的不改（账号回来了还是它）
struct AccountShelf: Equatable {
    /// 认得的工具（照 cca 的 TOOLS：claude、codex、agy）和档位（cca 的 plan_tier，一档比一档高）
    static let knownTools = ["claude", "codex", "agy"]
    static let knownTiers = ["entry", "standard", "advanced", "premium"]

    /// 有账号的工具：Claude、ChatGPT（codex）、Antigravity 的先后，认不出的新工具照出现的先后排在后面
    let tools: [String]
    /// 现在看的工具：记着的那个还有账号就是它，不然是头一个
    let tool: String
    /// 只看的那一档（空的是全部）：所有工具的账号加起来只有一档时没得筛、记着的那一档哪个工具都没有账号了，都算全部。
    /// 换工具时不变：换到的工具没有这一档的账号时列表是空的，页头说一句
    let tier: String
    /// 这个工具的账号（各台电脑的接在一起，不管档位）：各档后面的个数照它数
    let ofTool: [AccountStatus]

    init(_ all: [AccountStatus], tool wantedTool: String, tier wantedTier: String) {
        var tools = Self.knownTools.filter { t in all.contains { $0.tool == t } }
        for a in all where !a.tool.isEmpty && !tools.contains(a.tool) { tools.append(a.tool) }
        self.tools = tools
        let tool = tools.contains(wantedTool) ? wantedTool : tools.first ?? ""
        self.tool = tool
        let tiers = Self.knownTiers.filter { t in all.contains { $0.tier == t } }
        tier = tiers.count > 1 && tiers.contains(wantedTier) ? wantedTier : ""
        ofTool = all.filter { $0.tool == tool }
    }

    /// 筛的胶囊（「全部」之外的，入门 → 尊贵）：这个工具的账号里有的档位，加上筛着的那一档（这个工具里没有也列出来，看得到在筛什么）。
    /// 少于两档、又没在筛时是空的：没得筛，不放筛的胶囊
    var tierChips: [String] {
        let mine = Self.knownTiers.filter { t in t == tier || ofTool.contains { $0.tier == t } }
        return mine.count > 1 || !tier.isEmpty ? mine : []
    }

    /// 列出来的：这个工具、这一档的账号（页头的点名照它数）
    var shown: [AccountStatus] { ofTool.filter(matches) }

    func matches(_ a: AccountStatus) -> Bool {
        a.tool == tool && (tier.isEmpty || a.tier == tier)
    }

    /// 这个工具里这一档有几个账号（空的是这个工具的全部）
    func count(_ t: String) -> Int {
        t.isEmpty ? ofTool.count : ofTool.filter { $0.tier == t }.count
    }

    /// 一台电脑上要列的：这个工具、这一档的；官方账号在前、第三方 API 在后（和 Mac App 侧栏一样），各自照那台电脑给的先后
    func rows(_ accounts: [AccountStatus]) -> [AccountStatus] {
        let mine = accounts.filter(matches)
        return mine.filter { !$0.isAPI } + mine.filter(\.isAPI)
    }
}

extension PeerListing {
    /// 这台电脑上用这个账号的会话（会话的说明里的账号摘要对得上；那台电脑没给摘要时照工具和名字认）
    func sessions(of a: AccountStatus) -> [RemoteSession] {
        sessions.filter { s in
            if !a.aid.isEmpty, !s.info.aid.isEmpty { return s.info.aid == a.aid }
            return s.info.tool == a.tool && s.info.name == a.name && !a.name.isEmpty
        }
    }
}
