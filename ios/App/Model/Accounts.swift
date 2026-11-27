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

/// 「账号」页上面的工具切换和按套餐筛（照 Mac App 侧栏、菜单栏面板的工具切换和 Team 筛选）：只看一个工具的账号，可以只看一种套餐。
/// 只管显示；选的记在 AppModel（accountTool、accountPlan）。记着的工具、套餐已经没有账号了时照默认的看，记着的不改（账号回来了还是它）
struct AccountShelf: Equatable {
    /// 认得的工具（照 cca 的 TOOLS：claude、codex、agy）和档位（cca 的 plan_tier，一档比一档高：只用来给套餐排先后、画小徽记，界面上不写）
    static let knownTools = ["claude", "codex", "agy"]
    static let knownTiers = ["entry", "standard", "advanced", "premium"]

    /// 筛的一个胶囊：套餐照 cca 给的原名（Pro、Max、Plus、Google AI Pro……），档位是前面的小徽记
    struct PlanChip: Hashable {
        let plan: String
        let tier: String
    }

    /// 有账号的工具：Claude、ChatGPT（codex）、Antigravity 的先后，认不出的新工具照出现的先后排在后面
    let tools: [String]
    /// 现在看的工具：记着的那个还有账号就是它，不然是头一个
    let tool: String
    /// 只看的套餐（空的是全部）：这个工具的账号里有它、又不止一种套餐时才筛，不然算全部。换到没有这个套餐的工具时看全部，换回来还筛着
    let plan: String
    /// 这个工具的账号（各台电脑的接在一起，不管套餐）：各个套餐后面的个数照它数
    let ofTool: [AccountStatus]
    /// 筛的胶囊（「全部」之外的）：这个工具的账号里有的套餐；只有一种套餐时是空的（没得筛，不放）
    let planChips: [PlanChip]

    init(_ all: [AccountStatus], tool wantedTool: String, plan wantedPlan: String) {
        var tools = Self.knownTools.filter { t in all.contains { $0.tool == t } }
        for a in all where !a.tool.isEmpty && !tools.contains(a.tool) { tools.append(a.tool) }
        self.tools = tools
        let tool = tools.contains(wantedTool) ? wantedTool : tools.first ?? ""
        self.tool = tool
        let ofTool = all.filter { $0.tool == tool }
        self.ofTool = ofTool
        let chips = Self.plans(ofTool)
        planChips = chips.count > 1 ? chips : []
        plan = planChips.contains { $0.plan == wantedPlan } ? wantedPlan : ""
    }

    /// 这些账号里有的套餐：档位从低到高，同一档照名字（Pro 100、Pro 200 按数字排），认不出档位的排在最后。
    /// 几台电脑上的 cca 给同一个套餐的档位不一样（版本不同）时用认得的那个
    static func plans(_ accounts: [AccountStatus]) -> [PlanChip] {
        var tiers: [String: String] = [:]
        for a in accounts where !a.plan.isEmpty {
            if !knownTiers.contains(tiers[a.plan] ?? "") { tiers[a.plan] = a.tier }
        }
        func rank(_ t: String) -> Int { knownTiers.firstIndex(of: t) ?? knownTiers.count }
        return tiers.map { PlanChip(plan: $0.key, tier: $0.value) }.sorted {
            rank($0.tier) != rank($1.tier) ? rank($0.tier) < rank($1.tier) : $0.plan.localizedStandardCompare($1.plan) == .orderedAscending
        }
    }

    /// 列出来的：这个工具、这个套餐的账号（页头的点名照它数）
    var shown: [AccountStatus] { ofTool.filter(matches) }

    func matches(_ a: AccountStatus) -> Bool {
        a.tool == tool && (plan.isEmpty || a.plan == plan)
    }

    /// 这个工具里这个套餐有几个账号（空的是这个工具的全部）
    func count(_ p: String) -> Int {
        p.isEmpty ? ofTool.count : ofTool.filter { $0.plan == p }.count
    }

    /// 一台电脑上要列的：这个工具、这个套餐的；官方账号在前、第三方 API 在后（和 Mac App 侧栏一样），各自照那台电脑给的先后
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
