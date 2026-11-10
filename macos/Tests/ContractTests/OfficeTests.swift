import Foundation

// 办公室（App 的首页）：套餐档位、命令行会话的解码，员工状态（Diagnosis → Employee）的优先级，点名，文字，演示数据的时间平移；
// 周结算提醒（use_first：cca 给区间，App 只拿当前时间比；Diagnosis 里只在没有挡路的问题时出现，排在最前）。

private let now0 = 1794042000.0

/// 手写一个账号（默认：登录着的 Claude 官方账号、出口检测正常、什么都没开）；extra 里的值覆盖默认，NSNull() 表示 null。
private func acct(_ extra: [String: Any] = [:], tool: ToolID = .claude) -> Account {
    var d: [String: Any] = [
        "key": "a@example.com", "kind": "oauth", "login": "logged_in", "plan": "Max", "plan_tier": "advanced",
        "can": ["app": true, "web": true, "usage": true, "check": true],
        "health": ["version": 1, "checked_at": now0 - 600, "age_seconds": 600, "stale": false, "applies": true, "result": "ok",
                   "leak": ["dns": "ok", "webrtc": "ok"]],
    ]
    for (k, v) in extra { d[k] = v }
    return Account.sample(d, tool: tool)
}

/// 用量：5 小时、本周的百分比和重置时间（nil 表示没有这个窗口）；outdated 是 Claude Code 记的过时的套餐（cca 的 outdated_login_plan），
/// error 是查询出错的代码，useFirst 是 cca 给的周结算提醒的区间（use_first）。
private func usage(five: Double?, week: Double? = 10, stale: Bool = false, fiveResets: Double = now0 + 3600,
                   weekResets: Double = now0 + 86400 * 3, opus: Double? = nil, outdated: Any? = nil,
                   error: String? = nil, useFirst: Any? = nil) -> UsageEntry {
    var d: [String: Any] = ["key": "a@example.com", "source": "cache", "stale": stale]
    if let five { d["five_hour"] = ["utilization": five, "resets_epoch": fiveResets, "reset": false] }
    if let week { d["seven_day"] = ["utilization": week, "resets_epoch": weekResets, "reset": false] }
    if let opus { d["seven_day_opus"] = ["utilization": opus, "resets_epoch": weekResets, "reset": false] }
    if let outdated { d["outdated_login_plan"] = outdated }
    if let error { d["error_code"] = error }
    if let useFirst { d["use_first"] = useFirst }
    return try! JSONDecoder().decode(UsageEntry.self, from: JSONSerialization.data(withJSONObject: d))
}

/// 周结算提醒的区间：本周还剩 59%，一小时前进了区间，20 小时后重置
private let useFirstWindow: [String: Any] = ["left_percent": 59, "from_epoch": now0 - 3600, "until_epoch": now0 + 20 * 3600]

private func slot(_ state: String) -> LiveSlot {
    let d: [String: Any] = ["slot": 1, "group": "CCA-1", "port": 7901, "tool": "claude", "key": "a@example.com", "state": state]
    return try! JSONDecoder().decode(LiveSlot.self, from: JSONSerialization.data(withJSONObject: d))
}

private func health(_ result: String, stale: Bool = false, applies: Bool = true, region: String? = nil, dns: String = "ok") -> [String: Any] {
    var h: [String: Any] = ["version": 1, "checked_at": now0 - 600, "age_seconds": 600, "stale": stale, "applies": applies,
                            "result": result, "leak": ["dns": dns, "webrtc": "ok"]]
    if let region { h["restricted_region"] = region }
    return h
}

private func mood(_ a: Account, _ u: UsageEntry? = nil, _ live: LiveSlot? = nil, now: Double = now0) -> Employee.Mood {
    Employee.make(a, usage: u, live: live, now: now).mood
}

func officeTests() async {
    section("办公室")

    // 套餐档位、命令行会话的解码（旧版 cca 没有这两个字段）
    check("plan_tier", acct(["plan_tier": "premium"]).planTier == .premium && acct(["plan_tier": NSNull()]).planTier == .unknown
          && acct(["plan_tier": ""]).planTier == .unknown && !acct(["plan_tier": "zz"]).planTier.isKnown
          && PlanTier.unknown.isKnown)
    let none = acct()
    check("没有 cli：没有会话、不在用", none.cli == nil && none.cliSessions == 0 && !none.isWorking)
    let two = acct(["cli": ["running": true, "sessions": 2, "since_epoch": now0 - 60]])
    check("cli：两个会话", two.cliSessions == 2 && two.cli?.sinceEpoch == now0 - 60 && two.isWorking)
    check("cli：只给了 sessions 也算在运行", acct(["cli": ["sessions": 3]]).cliSessions == 3)
    check("cli：在运行但 sessions 是 0 当作 1 个", acct(["cli": ["running": true, "sessions": 0]]).cliSessions == 1)
    check("cli：不在运行时 sessions 不算、没有开始时间", acct(["cli": ["running": false, "sessions": 2, "since_epoch": now0]]).cliSessions == 0
          && acct(["cli": ["running": false, "sessions": 2, "since_epoch": now0]]).cliSince == nil)
    check("cli：类型不对当作没有", acct(["cli": ["running": "yes", "sessions": "abc"]]).cliSessions == 0)
    check("App 开着也算在用", acct(["app": ["running": true, "pid": 1]]).isWorking)
    check("旧版 cca 没有 can.billing：当作不能，不显示账单页入口", !acct().can.billing && acct().can.web)
    check("旧版 cca 没有 can.limit_resets：当作不能，不显示用量页入口", !acct().can.limitResets)

    // Diagnosis：顺序就是账号页那句话的顺序
    check("没登录：只说这一件", Diagnosis.all(acct(["login": "logged_out", "health": NSNull()]), usage: nil, live: slot("closed"), now: now0)
          == [.loggedOut])
    check("登错号", Diagnosis.all(acct(["login": "email_mismatch", "login_email": "eve@example.com"]), usage: nil, live: nil, now: now0)
          == [.emailMismatch(loginEmail: "eve@example.com")])
    let missing = acct(["can": ["app": false, "web": true, "usage": true, "check": true], "can_why": ["app": "app_missing"]])
    let order = Diagnosis.all(missing, usage: usage(five: 100), live: slot("closed"), now: now0)
    check("没装 App 只是提示：排在端口、额度这些真问题后面", order.count == 3 && order.first == .portClosed && order.last == .appMissing,
          "\(order)")
    check("选错节点", Diagnosis.all(acct(), usage: nil, live: slot("drift"), now: now0) == [.portDrift])
    check("出口在不可用地区", Diagnosis.all(acct(["health": health("restricted", region: "中国大陆")]), usage: nil, live: nil, now: now0)
          == [.exitBad(.restricted, region: "中国大陆")])
    check("旧的、换了节点的检测结果不算", Diagnosis.all(acct(["health": health("restricted", stale: true)]), usage: nil, live: nil, now: now0).isEmpty
          && Diagnosis.all(acct(["health": health("no_exit", applies: false)]), usage: nil, live: nil, now: now0).isEmpty)
    check("纯净度低、泄露、从没检测过", Diagnosis.all(acct(["health": health("low_purity", dns: "leak")]), usage: nil, live: nil, now: now0)
          == [.lowPurity, .leak] && Diagnosis.all(acct(["health": NSNull()]), usage: nil, live: nil, now: now0) == [.neverChecked])
    func pressed(_ u: UsageEntry, now: Double = now0) -> [Diagnosis.QuotaPressure] { Diagnosis.quotaPressure(u, now: now) }
    check("额度：快用完（≥90%）要数据是新的", pressed(usage(five: 95)).map(\.exhausted) == [false] && pressed(usage(five: 95, stale: true)).isEmpty)
    check("额度：用完了不看数据旧不旧", pressed(usage(five: 100, stale: true)).map(\.exhausted) == [true])
    check("额度：显示成 100% 才算用完", pressed(usage(five: 99.4)).map(\.exhausted) == [false] && pressed(usage(five: 99.5)).map(\.exhausted) == [true])
    check("额度：重置过了不算", pressed(usage(five: 100), now: now0 + 7200).isEmpty)
    check("额度：模型级的窗口不算", pressed(usage(five: 10, opus: 100)).isEmpty)
    check("额度：5 小时在前", pressed(usage(five: 100, week: 100)).map(\.window) == [.fiveHour, .week])
    check("额度：挡住账号的是用完的里面最晚重置的那个（账号页和工位一样说几点回来）",
          pressed(usage(five: 100, week: 100)).blocking?.window == .week && pressed(usage(five: 100, week: 50)).blocking?.window == .fiveHour
          && pressed(usage(five: 95)).blocking == nil)
    let both = Diagnosis.all(acct(["health": health("low_purity")]), usage: usage(five: 92), live: nil, now: now0)
    if case .quota? = both.first { check("额度排在纯净度前面", both.last == .lowPurity && both.count == 2) } else { check("额度排在纯净度前面", false, "\(both)") }
    check("Claude Code 记的套餐过时了：只有这一件时就说它", Diagnosis.all(acct(), usage: usage(five: 10, outdated: "Pro"), live: nil, now: now0)
          == [.outdatedLoginPlan("Pro")])
    let plan = Diagnosis.all(missing, usage: usage(five: 100, outdated: "Pro"), live: nil, now: now0)
    if case .quota? = plan.first {
        check("套餐过时排在额度后面、没装 App 前面（照常能用，不算真问题）", plan.count == 3 && plan[1] == .outdatedLoginPlan("Pro") && plan[2] == .appMissing, "\(plan)")
    } else {
        check("套餐过时排在额度后面、没装 App 前面（照常能用，不算真问题）", false, "\(plan)")
    }
    check("对得上（null）、空的、类型不对都不算", Diagnosis.all(acct(), usage: usage(five: 10, outdated: NSNull()), live: nil, now: now0).isEmpty
          && usage(five: 10, outdated: "").outdatedLoginPlan == nil && usage(five: 10, outdated: 3).outdatedLoginPlan == nil)

    // 员工的状态
    check("没登录：椅子空着", mood(acct(["login": "logged_out", "app": ["running": true]])) == .absent)
    check("登错号：坐着别人", mood(acct(["login": "email_mismatch", "login_email": "eve@example.com"])) == .wrongPerson(loginEmail: "eve@example.com"))
    check("端口连不上", mood(acct(), nil, slot("closed")) == .unwell(.portClosed) && Employee.Ailment.portClosed.severe)
    check("选错节点（橙）", mood(acct(), nil, slot("drift")) == .unwell(.wrongNode) && !Employee.Ailment.wrongNode.severe)
    check("出口有问题", mood(acct(["health": health("restricted", region: "中国大陆")])) == .unwell(.exit(.restricted, region: "中国大陆")))
    check("额度用完：休息到最晚那个窗口重置", mood(acct(), usage(five: 100, week: 100)) == .resting(until: now0 + 86400 * 3))
    check("线路有问题比休息要紧", mood(acct(), usage(five: 100), slot("closed")) == .unwell(.portClosed))
    check("时间到了就回来", mood(acct(["app": ["running": true]]), usage(five: 100), now: now0 + 7200) == .working)
    check("App 开着：在忙", mood(acct(["app": ["running": true]])) == .working)
    check("命令行里在用：在忙", mood(acct(["cli": ["running": true, "sessions": 1, "since_epoch": now0]])) == .working)
    check("什么都没开：空闲", mood(acct()) == .idle)
    let sick = Employee.make(acct(["app": ["running": true]]), usage: nil, live: slot("closed"), now: now0)
    check("有毛病时也记得 App 开着", sick.app && sick.isWorking && sick.mood == .unwell(.portClosed))
    let near = Employee.make(acct(["health": health("ok", dns: "leak")]), usage: usage(five: 92), live: nil, now: now0)
    check("快用完、泄露是小标记，不改头条", near.mood == .idle && near.flags == [.nearLimit, .leak])
    check("没装 App、没检测过的小标记", Employee.make(missing, usage: nil, live: nil, now: now0).flags == [.noApp]
          && Employee.make(acct(["health": NSNull()]), usage: nil, live: nil, now: now0).flags == [.neverChecked])
    let outdated = Employee.make(acct(["app": ["running": true]]), usage: usage(five: 10, outdated: "Pro"), live: nil, now: now0)
    check("Claude Code 记的套餐过时了：照常在忙，不改头条、没有小标记", outdated.mood == .working && outdated.flags.isEmpty)
    check("套餐过时的一句（不知道账号现在的套餐时只说前半句）", Texts.outdatedLoginPlan("Pro", current: "Max") == "Claude Code 里记的套餐还是 Pro，账号现在是 Max"
          && Texts.outdatedLoginPlan("Pro", current: "") == "Claude Code 里记的套餐还是 Pro" && Texts.outdatedLoginPlan("Max", current: nil) == "Claude Code 里记的套餐还是 Max")
    let api = acct(["key": "kimi", "kind": "api", "login": "api", "plan": "", "plan_tier": "", "health": NSNull(),
                    "cli": ["running": true, "sessions": 1, "since_epoch": now0 - 120]])
    let contractor = Employee.make(api, usage: nil, live: nil, now: now0)
    check("第三方 API 账号是外包，命令行里在用就在忙", contractor.department == .contractors && contractor.mood == .working
          && contractor.sessions == 1 && contractor.since == now0 - 120 && contractor.tier == .unknown)
    check("Codex 的账号在 ChatGPT 组", Employee.make(acct(tool: .codex), usage: nil, live: nil, now: now0).department == .chatgpt)

    // 文字
    var utc = Calendar(identifier: .gregorian)
    utc.timeZone = TimeZone(identifier: "UTC")!
    var sh = Calendar(identifier: .gregorian)
    sh.timeZone = TimeZone(identifier: "Asia/Shanghai")!
    check("几点恢复：今天", Texts.backAt(epoch: 1794050010, now: now0, calendar: utc) == "11:13 恢复"
          && Texts.backAt(epoch: 1794050010, now: now0, calendar: sh) == "19:13 恢复", Texts.backAt(epoch: 1794050010, now: now0, calendar: sh))
    check("几点恢复：明天、再远写星期几", Texts.backTime(epoch: now0 + 86400, now: now0, calendar: utc) == "明天 09:00"
          && Texts.backTime(epoch: 1794297600, now: now0, calendar: sh) == "周二 16:00", Texts.backTime(epoch: 1794297600, now: now0, calendar: sh))
    check("几点恢复：时间过了", Texts.backAt(epoch: now0 - 1, now: now0) == "马上恢复")
    check("在忙什么", Texts.working(app: true, appLabel: "Claude App", sessions: 2, since: now0 - 720, now: now0)
          == "Claude App 开着 · 终端里 2 个会话，已 12 分钟"
          && Texts.working(app: false, appLabel: "", sessions: 1, since: now0 - 10, now: now0) == "终端里在用，刚开始"
          && Texts.working(app: true, appLabel: "ChatGPT App", sessions: 0, since: nil, now: now0) == "ChatGPT App 开着")
    let resting = Employee.make(acct(), usage: usage(five: 100, week: 20, fiveResets: 1794050010), live: nil, now: now0)
    check("工位下面那一行", Texts.mood(resting, appLabel: "Claude App", now: now0).hasPrefix("额度用完了，")
          && Texts.mood(Employee.make(acct(["login": "logged_out"]), usage: nil, live: nil, now: now0), appLabel: "", now: now0) == "还没登录"
          && Texts.mood(sick, appLabel: "", now: now0) == "端口连不上")
    check("档位的叫法", Texts.planTier(.premium) == "尊贵" && Texts.planTier(.entry) == "入门" && Texts.planTier(.unknown) == nil)
    check("读屏念的一整句", Texts.deskLabel(Employee.make(acct(["key": "工作(bob@example.com)", "name": "工作", "plan": "Ultra",
          "plan_tier": "premium", "app": ["running": true]]), usage: nil, live: nil, now: now0), appLabel: "Claude App", now: now0)
          == "工作，Claude 账号，Ultra，尊贵套餐，Claude App 开着")
    let oneReset = try? JSONDecoder().decode(ResetCredits.self, from: Data(#"{"availableCount":1}"#.utf8))
    check("读屏也念还能重置几次", Texts.deskLabel(Employee.make(acct(), usage: nil, live: nil, now: now0), appLabel: "", now: now0, resets: oneReset)
          .hasSuffix("，还能重置额度 1 次"))
    check("工位上的短写法", Texts.mood(Employee.make(two, usage: nil, live: nil, now: now0), appLabel: "", now: now0, brief: true)
          == "在用，已 1 分钟")

    // 手写的演示数据：每种状态都有
    if let d = fixture("state.json"), let s = try? DocumentDecoder.decode(StateDoc.self, from: d) {
        let o = Office.make(tools: s.tools, codexEnabled: true, usage: { $0.usage }, live: { _ in nil }, now: now0)
        check("三个组：Claude 5 人、ChatGPT 1 人、外包 2 人", o.rooms.map(\.department) == [.claude, .chatgpt, .contractors]
              && o.rooms.map(\.people.count) == [5, 1, 2])
        let moods = Dictionary(uniqueKeysWithValues: o.people.map { ($0.account.key, $0.mood) })
        check("每个人的状态", moods["工作(bob@example.com)"] == .working && moods["carol@example.com"] == .absent
              && moods["测试(dave@example.com)"] == .wrongPerson(loginEmail: "eve@example.com")
              && moods["erin@example.com"] == .resting(until: 1794050010)
              && moods["grace@example.com"] == .unwell(.exit(.restricted, region: "中国大陆"))
              && moods["直连接口"] == .idle && moods["香港接口"] == .working && moods["frank@example.com"] == .idle,
              "\(moods)")
        check("点名", Texts.officeSummary(o.census) == "8 个账号 · 2 个在用 · 2 个空闲 · 1 个额度用完 · 1 个没登录 · 1 个登错号 · 1 个有问题"
              && Texts.officeSummary(o.census, brief: true) == "8 个账号 · 2 个在用"
              && o.census.needsAttention == 3, Texts.officeSummary(o.census))
        check("没启用 Codex：没有 ChatGPT 组", Office.make(tools: s.tools, codexEnabled: false, usage: { $0.usage }, live: { _ in nil },
                                                    now: now0).rooms.map(\.department) == [.claude, .contractors])
        if let ld = fixture("live_unreachable.json"), let live = try? DocumentDecoder.decode(LiveDoc.self, from: ld) {
            let o2 = Office.make(tools: s.tools, codexEnabled: true, usage: { $0.usage }, live: { live.slot(for: $0) }, now: now0)
            let m2 = Dictionary(uniqueKeysWithValues: o2.people.map { ($0.account.key, $0.mood) })
            check("路由器连不上：走路由器的人都端口不通（没登录、登错号的照旧，直连的不受影响）",
                  ["工作(bob@example.com)", "erin@example.com", "grace@example.com", "香港接口", "frank@example.com"]
                    .allSatisfy { m2[$0] == .unwell(.portClosed) } && m2["carol@example.com"] == .absent
                  && m2["测试(dave@example.com)"] == .wrongPerson(loginEmail: "eve@example.com") && m2["直连接口"] == .idle, "\(m2)")
        }
    } else {
        check("读 state.json", false)
    }
    check("空的总览", Texts.officeSummary(OfficeCensus([])) == "还没有账号"
          && Office.make(tools: [], codexEnabled: false, usage: { _ in nil }, live: { _ in nil }, now: now0).rooms.map(\.department) == [.claude])

    // 演示数据的时间平移到“现在”（测试不平移）
    let shifted = FixtureTransport(directory: fixtureDir, stepDelay: 0, rebaseTo: 1_800_000_000)
    let delta = 1_800_000_000 - now0
    if let r = try? await shifted.send(CcaRequest(arguments: ["_gui", "state"], timeout: 5), onEvent: nil),
       let s = try? DocumentDecoder.decode(StateDoc.self, from: r.stdout),
       let bob = s.account(AccountRef(tool: .claude, key: "工作(bob@example.com)")) {
        check("平移：generated_at 落在现在，*_epoch、checked_at 一起挪，age_seconds 不动",
              s.generatedAt == 1_800_000_000 && bob.usage?.fiveHour?.resetsEpoch == 1794050010 + delta
              && bob.cli?.sinceEpoch == 1794041280 + delta && bob.health?.checkedAt == 1794041000 + delta
              && bob.health?.ageSeconds == 1000 && bob.usage?.stale == false && bob.planTier == .premium)
    } else {
        check("平移：读得到 state", false)
    }
    if let r = try? await FixtureTransport(directory: fixtureDir, stepDelay: 0).send(CcaRequest(arguments: ["_gui", "state"], timeout: 5),
                                                                                  onEvent: nil) {
        check("不平移：原样", r.stdout == fixture("state.json"))
    }
}

func useFirstTests() {
    section("周结算提醒")
    // 本周 41%、20 小时后重置；cca 给的区间默认是 useFirstWindow
    func entry(five: Double = 10, stale: Bool = false, error: String? = nil, outdated: String? = nil,
               useFirst: Any? = useFirstWindow) -> UsageEntry {
        usage(five: five, week: 41, stale: stale, weekResets: now0 + 20 * 3600, outdated: outdated, error: error, useFirst: useFirst)
    }
    let f = UseFirst(leftPercent: 59, fromEpoch: now0 - 3600, untilEpoch: now0 + 20 * 3600)
    check("use_first：解码", entry().useFirst == f)
    check("use_first：null、缺字段、区间是空的都算没有", entry(useFirst: NSNull()).useFirst == nil
          && entry(useFirst: ["left_percent": 59]).useFirst == nil
          && entry(useFirst: ["left_percent": 59, "from_epoch": now0, "until_epoch": now0]).useFirst == nil)
    check("use_first：整数、浮点数、写成字符串的数字都收",
          entry(useFirst: ["left_percent": 59.0, "from_epoch": Int(now0), "until_epoch": "\(now0 + 10)"]).useFirst?.leftPercent == 59)
    check("isActive：[from, until)", !f.isActive(now: f.fromEpoch - 1) && f.isActive(now: f.fromEpoch)
          && f.isActive(now: f.untilEpoch - 1) && !f.isActive(now: f.untilEpoch))

    check("健康的账号、在区间里：只有这一条建议", Diagnosis.all(acct(), usage: entry(), live: nil, now: now0) == [.useFirst(f)])
    check("区间外（还没到重置前 24 小时、已经重置了）：没有",
          Diagnosis.all(acct(), usage: entry(), live: nil, now: f.fromEpoch - 1).isEmpty
          && Diagnosis.all(acct(), usage: entry(), live: nil, now: f.untilEpoch).isEmpty)
    check("旧版 cca（没有 use_first）：和以前一样", Diagnosis.all(acct(), usage: entry(useFirst: nil), live: nil, now: now0).isEmpty)

    // 挡路的：用不了、额度紧张、出口有风险的都不建议先用；其余的问题和没有 use_first 时一模一样
    let blockers: [(String, Account, (Any?) -> UsageEntry, LiveSlot?)] = [
        ("没登录", acct(["login": "logged_out"]), { entry(useFirst: $0) }, nil),
        ("登错号", acct(["login": "email_mismatch"]), { entry(useFirst: $0) }, nil),
        ("登录失效", acct(), { entry(error: "auth", useFirst: $0) }, nil),
        ("端口连不上", acct(), { entry(useFirst: $0) }, slot("closed")),
        ("节点选错", acct(), { entry(useFirst: $0) }, slot("drift")),
        ("出口在不可用地区", acct(["health": health("restricted")]), { entry(useFirst: $0) }, nil),
        ("纯净度偏低", acct(["health": health("low_purity")]), { entry(useFirst: $0) }, nil),
        ("泄露风险", acct(["health": health("ok", dns: "leak")]), { entry(useFirst: $0) }, nil),
        ("5 小时用完了", acct(), { entry(five: 100, useFirst: $0) }, nil),
        ("5 小时快用完了", acct(), { entry(five: 92, useFirst: $0) }, nil),
        ("5 小时快用完了（旧数据也算）", acct(), { entry(five: 92, stale: true, useFirst: $0) }, nil),
    ]
    for (name, a, u, l) in blockers {
        let with = Diagnosis.all(a, usage: u(useFirstWindow), live: l, now: now0)
        let without = Diagnosis.all(a, usage: u(nil), live: l, now: now0)
        check("挡住建议：\(name)", !with.contains(.useFirst(f)) && with == without, "\(with)")
    }
    // 只是提示的（没检测过出口、没装 App、套餐过时）不挡：建议排在最前，提示跟在后面
    check("没检测过出口：建议在前", Diagnosis.all(acct(["health": NSNull()]), usage: entry(), live: nil, now: now0) == [.useFirst(f), .neverChecked])
    check("Claude Code 记的套餐过时了：建议在前", Diagnosis.all(acct(), usage: entry(outdated: "Pro"), live: nil, now: now0)
          == [.useFirst(f), .outdatedLoginPlan("Pro")])
    check("blocksUseFirst：建议本身不挡", !Diagnosis.useFirst(f).blocksUseFirst && Diagnosis.leak.blocksUseFirst && !Diagnosis.appMissing.blocksUseFirst)

    // 总览：头条、小标记、点名都不变，多一个沙漏
    let e = Employee.make(acct(), usage: entry(), live: nil, now: now0)
    let e0 = Employee.make(acct(), usage: entry(useFirst: nil), live: nil, now: now0)
    check("总览：头条、小标记都不变，卡片上多一个沙漏", e.mood == .idle && e.flags.isEmpty && e.useFirst == f && e0.useFirst == nil)
    check("总览：点名不变（建议先用不算要处理）", OfficeCensus([e]) == OfficeCensus([e0]) && OfficeCensus([e]).needsAttention == 0)
    let busy = Employee.make(acct(["app": ["running": true, "pid": 1]]), usage: entry(), live: nil, now: now0)
    check("在用的账号也照样提示", busy.mood == .working && busy.useFirst == f)

    var sh = Calendar(identifier: .gregorian)
    sh.timeZone = TimeZone(identifier: "Asia/Shanghai")!
    check("说法：明天几点重置", Texts.useFirst(f, now: now0, calendar: sh) == "本周额度还剩 59%，明天 13:00 重置，建议先用这个账号",
          Texts.useFirst(f, now: now0, calendar: sh))
    let today = UseFirst(leftPercent: 30, fromEpoch: now0 - 3600, untilEpoch: now0 + 4 * 3600)
    check("说法：今天几点重置", Texts.useFirst(today, now: now0, calendar: sh) == "本周额度还剩 30%，今天 21:00 重置，建议先用这个账号",
          Texts.useFirst(today, now: now0, calendar: sh))
    check("读屏：卡片那一整句带上建议", Texts.deskLabel(e, appLabel: "Claude App", now: now0).contains("建议先用这个账号"))
}
