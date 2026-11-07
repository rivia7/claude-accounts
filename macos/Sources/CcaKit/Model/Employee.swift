import Foundation

/// 办公室（App 的首页）里的一位员工：一个账号此刻的样子。全部从 cca 给的状态派生（经 Diagnosis，和账号页那句“下一步”
/// 共用一份优先级），App 不重算 cca 的规则。账号本身的事（哪个组、套餐档位、在不在用）直接读账号，这里只多出两样：
/// 头条状态（mood）和头条之外的小事（flags）。
struct Employee: Equatable, Sendable, Identifiable {
    /// 坐在哪个组：Claude 组、ChatGPT 组、外包（第三方 API 账号，两个工具的都在这里）
    enum Department: Hashable, Sendable, CaseIterable {
        case claude, chatgpt, contractors
    }

    /// 头条状态（哪个算头条由 Diagnosis.all 的顺序决定）
    enum Mood: Equatable, Sendable {
        /// 没登录：椅子空着
        case absent
        /// 登录的不是这个账号：工位上坐着别人
        case wrongPerson(loginEmail: String?)
        /// 端口连不上、节点选错、出口有问题
        case unwell(Ailment)
        /// 5 小时或本周的额度用完了：趴着睡觉，until 是用完的窗口里最晚重置的时间
        case resting(until: Double?)
        /// 在用：桌面 App 开着，或者有经 cca 启动的命令行会话
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

    var id: AccountRef { account.ref }
    var department: Department { Self.department(of: account) }
    var tier: PlanTier { account.planTier }
    /// 桌面 App 开着
    var app: Bool { account.appRunning }
    /// 经 cca 启动、还在运行的命令行会话
    var sessions: Int { account.cliSessions }
    /// 最早那个命令行会话开始的时间
    var since: Double? { account.cliSince }
    /// 在用（App 开着或者命令行里有会话；生病、休息时也照样记得）
    var isWorking: Bool { account.isWorking }

    static func department(of a: Account) -> Department {
        a.isAPI ? .contractors : a.tool == .codex ? .chatgpt : .claude
    }

    static func make(_ a: Account, usage: UsageEntry?, live: LiveSlot?, now: Double) -> Employee {
        var mood: Mood?
        var flags: [Flag] = []
        for d in Diagnosis.all(a, usage: usage, live: live, now: now) {
            switch d {
            case .loggedOut: mood = mood ?? .absent
            case .emailMismatch(let e): mood = mood ?? .wrongPerson(loginEmail: e)
            case .portClosed: mood = mood ?? .unwell(.portClosed)
            case .portDrift: mood = mood ?? .unwell(.wrongNode)
            case .exitBad(let r, let region): mood = mood ?? .unwell(.exit(r, region: region))
            case .quota(let windows):
                if let w = windows.blocking {
                    mood = mood ?? .resting(until: w.resetsEpoch)
                } else {
                    flags.append(.nearLimit)
                }
            case .appMissing: flags.append(.noApp)
            case .lowPurity: flags.append(.lowPurity)
            case .leak: flags.append(.leak)
            case .neverChecked: flags.append(.neverChecked)
            }
        }
        return Employee(account: a, mood: mood ?? (a.isWorking ? .working : .idle), flags: flags)
    }
}

extension Employee.Department {
    /// 在终端里添加这个组的账号（办公室各组的标题、工具栏的“＋”菜单）
    func addHandoff(codexPrefix: [String]) -> Handoff {
        switch self {
        case .claude: return .addAccount(prefix: [], product: "Claude")
        case .chatgpt: return .addAccount(prefix: codexPrefix, product: "ChatGPT")
        case .contractors: return .addAPI
        }
    }
}

/// 整间办公室：按组排好的员工（组里照账号表的顺序，座位不会因为状态变了就挪），和一行点名。
struct Office: Equatable, Sendable {
    struct Room: Equatable, Sendable, Identifiable {
        var department: Employee.Department
        var people: [Employee]
        var id: Employee.Department { department }
    }

    var rooms: [Room]

    var people: [Employee] { rooms.flatMap(\.people) }
    var census: OfficeCensus { OfficeCensus(people) }

    /// Claude 组一直有（空的也画出来，好让人知道从哪里添加）；ChatGPT 组在启用了 Codex 时有；外包有人才有。
    static func make(tools: [ToolState], codexEnabled: Bool, usage: (Account) -> UsageEntry?,
                     live: (AccountRef) -> LiveSlot?, now: Double) -> Office {
        let people = tools.filter { $0.id == .claude || ($0.id == .codex && codexEnabled) }
            .flatMap(\.accounts)
            .map { Employee.make($0, usage: usage($0), live: live($0.ref), now: now) }
        let byDept = Dictionary(grouping: people, by: \.department)
        var rooms = [Room(department: .claude, people: byDept[.claude] ?? [])]
        if codexEnabled { rooms.append(Room(department: .chatgpt, people: byDept[.chatgpt] ?? [])) }
        if let c = byDept[.contractors], !c.isEmpty { rooms.append(Room(department: .contractors, people: c)) }
        return Office(rooms: rooms)
    }
}

/// 点名：几位员工，几个在忙、几个空闲……（办公室页顶上那一行、侧栏“办公室”那一行）
struct OfficeCensus: Equatable, Sendable {
    var total = 0, working = 0, idle = 0, resting = 0, absent = 0, wrongPerson = 0, unwell = 0

    init(_ people: [Employee]) {
        total = people.count
        for p in people {
            switch p.mood {
            case .working: working += 1
            case .idle: idle += 1
            case .resting: resting += 1
            case .absent: absent += 1
            case .wrongPerson: wrongPerson += 1
            case .unwell: unwell += 1
            }
        }
    }

    /// 要用户动手的：没登录、登错号、线路或出口有问题
    var needsAttention: Int { absent + wrongPerson + unwell }
}
