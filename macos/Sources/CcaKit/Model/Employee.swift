import Foundation

/// 办公室（App 的首页）里的一位员工：一个账号此刻的样子。全部从 cca 给的状态派生（经 Diagnosis，和账号页那句“下一步”
/// 共用一份优先级），App 不重算 cca 的规则。账号本身的事（哪个组、套餐档位、在不在用）直接读账号，这里只多出两样：
/// 头条状态（mood）和头条之外的小事（flags）。
struct Employee: Equatable, Sendable, Identifiable {
    /// 坐在哪个组：Claude 组、ChatGPT 组、Antigravity 组、外包（第三方 API 账号，各工具的都在这里）
    enum Department: Hashable, Sendable, CaseIterable {
        case claude, chatgpt, antigravity, contractors
    }

    /// 头条状态：在用（桌面 App 开着、命令行里有会话）时总是在用，有问题时问题跟在后面（trouble）；没在用时是 Diagnosis.all 里
    /// 排在最前的那个问题，没有问题是空闲
    enum Mood: Equatable, Sendable {
        /// 没登录：椅子空着
        case absent
        /// 登录的不是这个账号：工位上坐着别人
        case wrongPerson(loginEmail: String?)
        /// 登录失效了（改过密码之类）：要重新登录
        case expired
        /// 端口连不上、节点选错、出口有问题
        case unwell(Ailment)
        /// 5 小时或本周的额度用完了：趴着睡觉，until 是用完的窗口里最晚重置的时间
        case resting(until: Double?)
        /// 在用：桌面 App 开着，或者有经 cca 启动的命令行会话（有问题时问题在 trouble 里，照样算在用）
        case working
        /// 一切正常，没在用
        case idle
    }

    enum Ailment: Equatable, Sendable {
        case portClosed
        case wrongNode
        case exit(HealthResult, region: String?)

        /// 红（用不了）还是橙（选错节点，同步一下就好）
        var severe: Bool { self != .wrongNode }
    }

    /// 头条之外还值得提一句的（工位右上角的小标记）
    enum Flag: Hashable, Sendable {
        /// 额度快用完了（≥90%）
        case nearLimit
        /// 没装桌面 App
        case noApp
        case lowPurity
        case leak
        case neverChecked

        /// 橙色的（要注意），其余只是提示
        var warns: Bool { self == .nearLimit || self == .lowPurity || self == .leak }
    }

    let account: Account
    var mood: Mood
    /// 按 Diagnosis 的顺序（要紧的在前）
    var flags: [Flag]
    /// 本周额度快重置了、还剩不少，建议先用（不改头条、不算小标记；卡片上画一个沙漏）
    var useFirst: UseFirst? = nil
    /// 有问题时是哪一个（没登录、登错号、登录失效、线路或出口、额度用完：Diagnosis.all 里排在最前的那个），没问题是 nil。
    /// 没在用时它就是头条；在用时头条是在用，它跟在后面（“在用 · 端口连不上”），要处理的照样算进点名的“要处理”
    var trouble: Mood? = nil

    var id: AccountRef { account.ref }
    var department: Department { Self.department(of: account) }
    var tier: PlanTier { account.planTier }
    /// 桌面 App 开着
    var app: Bool { account.appRunning }
    /// 经 cca 启动、还在运行的命令行会话
    var sessions: Int { account.cliSessions }
    /// 最早那个命令行会话开始的时间
    var since: Double? { account.cliSince }
    /// 在用（App 开着或者命令行里有会话；生病、休息时也照样记得）：这台 Mac 上的
    var isWorking: Bool { account.isWorking }
    /// 别的设备上在用（它们的名字，cca 排好的先后）：这台 Mac 上没在用、别的设备上在用时头条也是在用，说是哪台（「在「Mac-mini」上在用」）
    var elsewhere: [String] = []
    /// 犯困（工位上的人头一点一点、头顶一个快没电的电池在闪）：额度快用完了（小标记里的 nearLimit）、头条是在用或空闲；
    /// 或者额度用完了、桌面 App / 命令行还开着（在用，额度用完跟在后面：不然从 92% 到用完，人反倒精神了）。
    /// 头条是别的（没登录、登错号、线路有问题、用完了趴着睡）时照那件事画
    var drowsy: Bool {
        switch mood {
        case .working:
            if case .resting = trouble { return true }
            return flags.contains(.nearLimit)
        case .idle: return flags.contains(.nearLimit)
        case .absent, .wrongPerson, .expired, .unwell, .resting: return false
        }
    }

    static func department(of a: Account) -> Department {
        a.isAPI ? .contractors : .official(a.tool)
    }

    /// elsewhere：别的设备上在用（`cca devices --json`，PROTOCOL 3.4：别的设备的 `state` 是 `in_use`），AppModel.elsewhere 给的
    static func make(_ a: Account, usage: UsageEntry?, live: LiveSlot?, now: Double, elsewhere: [String] = []) -> Employee {
        var mood: Mood?   // 有问题时是哪一个（trouble）
        var flags: [Flag] = []
        var useFirst: UseFirst?
        for d in Diagnosis.all(a, usage: usage, live: live, now: now) {
            switch d {
            case .loggedOut: mood = mood ?? .absent
            case .emailMismatch(let e): mood = mood ?? .wrongPerson(loginEmail: e)
            case .loginExpired: mood = mood ?? .expired
            case .portClosed: mood = mood ?? .unwell(.portClosed)
            case .portDrift: mood = mood ?? .unwell(.wrongNode)
            case .exitBad(let r, let region): mood = mood ?? .unwell(.exit(r, region: region))
            case .quota(let windows):
                if let w = windows.blocking {
                    mood = mood ?? .resting(until: w.resetsEpoch)
                } else {
                    flags.append(.nearLimit)
                }
            case .outdatedLoginPlan: break   // 照常能用：不改头条、不加小标记（账号页那句话里说）
            case .appMissing: flags.append(.noApp)
            case .lowPurity: flags.append(.lowPurity)
            case .leak: flags.append(.leak)
            case .neverChecked: flags.append(.neverChecked)
            case .useFirst(let f): useFirst = f
            }
        }
        // 在用就是在用（桌面 App 开着、命令行里有会话）：有问题也不改，问题跟在后面
        var e = Employee(account: a, mood: a.isWorking || !elsewhere.isEmpty ? .working : mood ?? .idle, flags: flags, useFirst: useFirst,
                         trouble: mood)
        e.elsewhere = elsewhere
        return e
    }
}

extension Employee.Mood {
    /// 要用户动手的问题：没登录、登错号、登录失效、线路或出口有问题（额度用完只是等，不算）
    var needsAttention: Bool {
        switch self {
        case .absent, .wrongPerson, .expired, .unwell: return true
        case .resting, .working, .idle: return false
        }
    }
}

extension Employee.Department {
    /// App 设置里记这个组的排序用的名字
    var key: String {
        switch self {
        case .claude: return "claude"
        case .chatgpt: return "chatgpt"
        case .antigravity: return "agy"
        case .contractors: return "api"
        }
    }

    /// 这一组是哪个工具的官方账号；第三方 API 账号的组各工具共用，是 nil
    var tool: ToolID? {
        switch self {
        case .claude: return .claude
        case .chatgpt: return .codex
        case .antigravity: return .agy
        case .contractors: return nil
        }
    }

    /// 这个工具的官方账号坐哪一组（认不出的工具算 Claude 组）
    static func official(_ t: ToolID) -> Employee.Department {
        t == .codex ? .chatgpt : t == .agy ? .antigravity : .claude
    }

    /// 添加这个组的账号（办公室各组的标题、工具栏的“＋”菜单）。prefix 是这个工具的命令前缀（`_gui state` 的 cli_prefix）
    func addHandoff(prefix: [String]) -> Handoff {
        switch self {
        case .claude: return .addAccount(prefix: prefix, product: "Claude")
        case .chatgpt: return .addAccount(prefix: prefix, product: "ChatGPT")
        case .antigravity: return .addAccount(prefix: prefix, product: "Antigravity")
        case .contractors: return .addAPI()
        }
    }

    func addHandoff(codexPrefix: [String]) -> Handoff {
        addHandoff(prefix: self == .chatgpt ? codexPrefix : tool?.defaultPrefix ?? [])
    }
}

/// 整间办公室：按组排好的员工（组里按用户排的顺序，没排过的照账号表的顺序，座位不会因为状态变了就挪），和一行点名。
struct Office: Equatable, Sendable {
    struct Room: Equatable, Sendable, Identifiable {
        var department: Employee.Department
        var people: [Employee]
        var id: Employee.Department { department }
    }

    var rooms: [Room]

    var people: [Employee] { rooms.flatMap(\.people) }
    var census: OfficeCensus { OfficeCensus(people) }

    /// 列出来的工具（listed：AppModel.listedTools，这一版 cca 有的）每个一组，空的也画出来（好让人知道从哪里添加），三种工具一样：
    /// 有账号的组在前（照 Claude、ChatGPT、Antigravity 的先后），接着第三方 API 的（有人才有），空着的组在最后。
    /// 3.7.12 及以前 Claude 组一直排在最前（空的也是），ChatGPT、Antigravity 组启用了才有。
    static func make(tools: [ToolState], listed: [ToolID], usage: (Account) -> UsageEntry?,
                     live: (AccountRef) -> LiveSlot?, now: Double, elsewhere: (AccountRef) -> [String] = { _ in [] },
                     arrange: (Employee.Department, [Employee]) -> [Employee] = { $1 }) -> Office {
        let on = Set(listed)
        let people = tools.filter { on.contains($0.id) }
            .flatMap(\.accounts)
            .map { Employee.make($0, usage: usage($0), live: live($0.ref), now: now, elsewhere: elsewhere($0.ref)) }
        let byDept = Dictionary(grouping: people, by: \.department)
        let official = ToolID.allKnown.filter { on.contains($0) }.map { t -> Room in
            let d = Employee.Department.official(t)
            return Room(department: d, people: arrange(d, byDept[d] ?? []))
        }
        var rooms = official.filter { !$0.people.isEmpty }
        if let c = byDept[.contractors], !c.isEmpty { rooms.append(Room(department: .contractors, people: arrange(.contractors, c))) }
        return Office(rooms: rooms + official.filter(\.people.isEmpty))
    }
}

/// 点名：几位员工，几个在忙、几个空闲……（办公室页顶上那一行、侧栏“办公室”那一行）
struct OfficeCensus: Equatable, Sendable {
    var total = 0, working = 0, idle = 0, resting = 0, absent = 0, wrongPerson = 0, expired = 0, unwell = 0

    init(_ people: [Employee]) {
        total = people.count
        for p in people {
            switch p.mood {
            case .working:
                working += 1
                if p.trouble?.needsAttention == true { workingTroubled += 1 }
            case .idle: idle += 1
            case .resting: resting += 1
            case .absent: absent += 1
            case .wrongPerson: wrongPerson += 1
            case .expired: expired += 1
            case .unwell: unwell += 1
            }
        }
    }

    /// 在用、同时有要用户动手的问题的（算在“在用”里，不算进上面几样）
    var workingTroubled = 0

    /// 要用户动手的：没登录、登错号、登录失效、线路或出口有问题（在用的也算）
    var needsAttention: Int { absent + wrongPerson + expired + unwell + workingTroubled }
}
