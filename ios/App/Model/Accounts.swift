import CcaRemoteKit
import Foundation

/// 「账号」页推进去的一个账号：哪台电脑（配对编号）上的哪个账号（这个配对里认这个账号的编号，AccountStatus.id）
struct AccountRef: Hashable {
    let pid: String
    let id: String
}

/// 挡住一个账号的问题（Mac App 的 Employee.Mood 里放上来的状态看得出的那几个；端口池的实况——端口连不上、节点选错了——那台电脑的
/// 界面 App 开着时才放上来）
enum AccountTrouble: Equatable {
    case loggedOut
    case wrongAccount
    case expired
    /// 端口池里这个账号的端口连不上（Mac App 的 unwell(.portClosed)，红）
    case portClosed
    /// 路由器给这个账号选错了节点（Mac App 的 unwell(.wrongNode)：同步一下路由器就好，橙）
    case wrongNode
    /// 出口有问题（最近一次出口检测：不可用的地区、拿不到出口 IP、端口无法连接、接口无法连接；Mac App 的 unwell(.exit)）
    case exit(result: String, region: String?)
    /// 5 小时或本周的额度用完了：until 是用完的窗口里最晚重置的时间
    case usedUp(until: Double?)

    /// 要人动手的（额度用完只是等）
    var needsAttention: Bool {
        if case .usedUp = self { return false }
        return true
    }
}

/// 头条之外还值得提一句的（卡片右上角的小标记，Mac App 的 Employee.Flag），照 Diagnosis 的先后
enum AccountFlag: Hashable {
    /// 额度快用完了（≥90%）
    case nearLimit
    /// 那台电脑上没装桌面 App
    case noApp
    case lowPurity
    case leak
    /// 从没检测过出口
    case neverChecked

    /// 橙色的（要注意），其余只是提示
    var warns: Bool { self == .nearLimit || self == .lowPurity || self == .leak }
}

/// 一个账号现在怎么样，照 Mac App 的总览（Employee、Diagnosis.all，先后也照它）：登录的问题（没登录、登错号、登录失效）挡住别的一切；
/// 不然端口连不上、节点选错了、出口有问题、额度用完是问题（在用时头条总是在用，问题跟在后面；没在用时是那个问题，没有问题是空闲），快用完、没装桌面 App、
/// 纯净度偏低、可能泄露、没检测过出口是小标记，套餐过时只在账号页那句话里说；建议先用（本周额度快重置了、还剩不少）只在没有挡路的问题
/// （端口、出口、额度、纯净度、泄露；套餐过时、没装 App、没检测过不挡）、5 小时的额度也不紧的时候
struct AccountMood: Equatable {
    let busy: Bool
    let trouble: AccountTrouble?
    /// 照 Diagnosis 的先后（要紧的在前）
    let flags: [AccountFlag]
    /// Claude Code 登录时记下的套餐过时了：记下的那个（不改头条、不加小标记）
    let outdatedPlan: String?
    let useFirst: UseFirst?

    var nearLimit: Bool { flags.contains(.nearLimit) }

    init(_ a: AccountStatus, now: Double) {
        busy = a.busy
        let login: AccountTrouble? = a.login == "logged_out" ? .loggedOut
            : a.login == "email_mismatch" ? .wrongAccount
            : a.usage?.errorCode == "auth" ? .expired : nil
        if let login {
            trouble = login
            flags = []
            outdatedPlan = nil
            useFirst = nil
            return
        }
        var trouble: AccountTrouble?
        var flags: [AccountFlag] = []
        // 端口池的实况（那台电脑的界面 App 查的）排在出口检测前面（Mac App 的 Diagnosis 一样）
        if a.port == "closed" { trouble = .portClosed } else if a.port == "drift" { trouble = .wrongNode }
        // 出口检测只看算数的那一份（现在的节点测的、没过时：Mac App 一样）
        let net = a.net?.current
        if trouble == nil, let net, net.exitBad { trouble = .exit(result: net.result, region: net.region) }
        if let b = a.blocking(now: now) {
            trouble = trouble ?? .usedUp(until: b.resetsEpoch)
        } else if a.nearLimit(now: now) {
            flags.append(.nearLimit)
        }
        outdatedPlan = a.usage?.outdatedLoginPlan
        if !a.isAPI, a.whyApp == "app_missing" { flags.append(.noApp) }
        if let net {
            if net.result == "low_purity" { flags.append(.lowPurity) }
            if net.hasLeak { flags.append(.leak) }
        }
        if a.net?.checked == false { flags.append(.neverChecked) }
        self.trouble = trouble
        self.flags = flags
        let blocked = trouble != nil || flags.contains { $0 == .nearLimit || $0 == .lowPurity || $0 == .leak }
        let f = a.usage?.useFirst.flatMap { $0.isActive(now: now) ? $0 : nil }
        useFirst = !blocked && !a.fiveHourPressed(now: now) ? f : nil
    }
}

/// 点名（「账号」页顶上那一行，Mac App 的 OfficeCensus）：几个账号，几个在用、几个空闲、几个额度用完……
struct AccountCensus: Equatable {
    var total = 0, working = 0, idle = 0, resting = 0, absent = 0, wrongPerson = 0, expired = 0, unwell = 0
    /// 在用、同时有要用户动手的问题的（算在「在用」里，不算进上面几样）
    var workingTroubled = 0

    /// 要用户动手的：没登录、登错号、登录失效、端口或出口有问题（在用的也算；额度用完只是等，不算）。底下「总览」的角标照它（Mac App 侧栏「N 个要处理」）
    var needsAttention: Int { absent + wrongPerson + expired + unwell + workingTroubled }

    init(_ all: [AccountStatus], now: Double) {
        total = all.count
        for a in all {
            let m = AccountMood(a, now: now)
            if m.busy {
                working += 1
                if m.trouble?.needsAttention == true { workingTroubled += 1 }
                continue
            }
            switch m.trouble {
            case nil: idle += 1
            case .usedUp: resting += 1
            case .loggedOut: absent += 1
            case .wrongAccount: wrongPerson += 1
            case .expired: expired += 1
            case .portClosed, .wrongNode, .exit: unwell += 1
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

    /// 按 Team 筛的一项（Mac App 的 TeamFilter）：全部账号、某个 Team 给的（照名字认：几台电脑上的同一个 Team 名字一样）、
    /// 不在 Team 里的（这台设备自己的账号、第三方 API 的）
    enum TeamPick: Hashable {
        case all
        case team(String)
        case outside

        /// 记在设置里的写法（和 Mac App 的 TeamFilter.stored 一个样子，只是 Team 照名字记）
        var stored: String {
            switch self {
            case .all: return ""
            case .team(let name): return "team:" + name
            case .outside: return "outside"
            }
        }

        init(stored s: String) {
            if s == "outside" {
                self = .outside
            } else if s.hasPrefix("team:"), s.count > 5 {
                self = .team(String(s.dropFirst(5)))
            } else {
                self = .all
            }
        }

        func includes(_ a: AccountStatus) -> Bool {
            switch self {
            case .all: return true
            case .team(let name): return a.team == name
            case .outside: return a.team == nil
            }
        }
    }

    /// 筛的一个胶囊：套餐照 cca 给的原名（Pro、Max、Plus、Google AI Pro……），档位是前面的小徽记
    struct PlanChip: Hashable {
        let plan: String
        let tier: String
    }

    /// 按 Team 筛的几项（「全部账号」之外的）：账号里有的 Team（照出现的先后）和「不在 Team 里的」；分不出两组时是空的（没得筛，不放）
    let teamPicks: [TeamPick]
    /// 只看的：记着的那一项还在 teamPicks 里时是它，不然是全部（记着的不改：那个 Team 的账号回来了还是它）
    let team: TeamPick
    /// 有账号的工具（筛着 Team 时只算这个 Team 的）：Claude、ChatGPT（codex）、Antigravity 的先后，认不出的新工具照出现的先后排在后面
    let tools: [String]
    /// 现在看的工具：记着的那个还有账号就是它，不然是头一个
    let tool: String
    /// 只看的套餐（空的是全部）：这个工具的账号里有它、又不止一种套餐时才筛，不然算全部。换到没有这个套餐的工具时看全部，换回来还筛着
    let plan: String
    /// 这个工具的账号（各台电脑的接在一起，不管套餐）：各个套餐后面的个数照它数
    let ofTool: [AccountStatus]
    /// 筛的胶囊（「全部」之外的）：这个工具的账号里有的套餐；只有一种套餐时是空的（没得筛，不放）
    let planChips: [PlanChip]

    init(_ all: [AccountStatus], tool wantedTool: String, plan wantedPlan: String, team wantedTeam: TeamPick = .all) {
        var names: [String] = []
        for a in all { if let t = a.team, !names.contains(t) { names.append(t) } }
        let picks = names.map(TeamPick.team) + (all.contains { $0.team == nil } ? [TeamPick.outside] : [])
        teamPicks = picks.count > 1 ? picks : []
        let team = teamPicks.contains(wantedTeam) ? wantedTeam : .all
        self.team = team
        let inTeam = all.filter(team.includes)
        var tools = Self.knownTools.filter { t in inTeam.contains { $0.tool == t } }
        for a in inTeam where !a.tool.isEmpty && !tools.contains(a.tool) { tools.append(a.tool) }
        self.tools = tools
        let tool = tools.contains(wantedTool) ? wantedTool : tools.first ?? ""
        self.tool = tool
        let ofTool = inTeam.filter { $0.tool == tool }
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
        team.includes(a) && a.tool == tool && (plan.isEmpty || a.plan == plan)
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
