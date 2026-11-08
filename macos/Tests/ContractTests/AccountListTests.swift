import Foundation

// 账号怎么排（拖动、置顶）、自动检测网络出口、登录失效后重新登录。

private func fixtureText(_ name: String) -> String? { fixture(name).map { String(decoding: $0, as: UTF8.self) } }

/// cca reorder 做的事：把 state 里这个工具的账号按这个顺序排。
private func reordered(_ state: String, tool: String, keys: [String]) -> String {
    guard var doc = (try? JSONSerialization.jsonObject(with: Data(state.utf8))) as? [String: Any], var tools = doc["tools"] as? [[String: Any]],
          let i = tools.firstIndex(where: { $0["id"] as? String == tool }), let list = tools[i]["accounts"] as? [[String: Any]] else { return state }
    func rank(_ a: [String: Any]) -> Int { keys.firstIndex(of: a["key"] as? String ?? "") ?? .max }
    tools[i]["accounts"] = list.sorted { rank($0) < rank($1) }
    doc["tools"] = tools
    return (try? JSONSerialization.data(withJSONObject: doc)).map { String(decoding: $0, as: UTF8.self) } ?? state
}

/// 测试中途换掉 transport 回的内容。
private final class Box: @unchecked Sendable {
    var value: String
    init(_ value: String) { self.value = value }
}

func arrangementTests() {
    section("账号的排序和置顶：规则")
    var a = AccountArrangement()
    let ids = ["a", "b", "c", "d"]
    func arranged() -> [String] { a.arranged(ids, group: "g", id: { $0 }) }
    check("没排过：照原来的顺序", arranged() == ids)
    a.setTop("c", true)
    check("置顶的排在最前面", arranged() == ["c", "a", "b", "d"])
    a.order["g"] = ["b", "a"]
    check("排过的按排的顺序，新添加的接在后面", arranged() == ["c", "b", "a", "d"])
    a.setTop("c", false)
    check("取消置顶：回到它原来的位置", arranged() == ["b", "a", "c", "d"])
    check("别的组不受影响", a.arranged(ids, group: "h", id: { $0 }) == ids)

    var m = AccountArrangement()
    m.move(group: "g", full: ["a", "b", "c"], shown: ["a", "b", "c"], from: [0], to: 3)
    check("往后挪到最后", m.order["g"] == ["b", "c", "a"])
    m.move(group: "g", full: ["b", "c", "a"], shown: ["b", "c", "a"], from: [2], to: 0)
    check("往前挪到最前", m.order["g"] == ["a", "b", "c"])
    m.move(group: "g", full: ["a", "b", "c"], shown: ["a", "b", "c"], from: [0], to: 2)
    check("挪到中间：落在目标位置的前面（和 onMove 一样）", m.order["g"] == ["b", "a", "c"])
    m.move(group: "g", full: ["a", "X", "b", "Y", "c"], shown: ["a", "b", "c"], from: [2], to: 0)
    check("只换这几个账号之间的先后，别的账号位置不动", m.order["g"] == ["c", "X", "a", "Y", "b"])
    let round = try? JSONDecoder().decode(AccountArrangement.self, from: JSONEncoder().encode(a))
    check("存得下读得回", round == a)
}

@MainActor func accountListModelTests() async {
    section("账号的排序和置顶：App")
    guard let hello = fixtureText("hello.json"), let state = fixtureText("state.json") else {
        check("夹具", false)
        return
    }
    let box = Box(state)
    let t = ScriptedTransport { request, _ in
        if request.arguments == ["_gui", "hello"] { return response(0, stdout: hello) }
        if request.arguments == ["_gui", "state"] { return response(0, stdout: box.value) }
        if let i = request.arguments.firstIndex(of: "reorder") {
            box.value = reordered(box.value, tool: i == 0 ? "claude" : "codex", keys: Array(request.arguments[(i + 1)...]))
            return response(0)
        }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    let prefs = MemoryPrefs()
    let m = makeModel(t, prefs: prefs)
    await m.start()
    func official() -> [String] { m.accountGroups(.claude)[0].accounts.map(\.key) }
    func table() -> [String] { m.accounts(.claude).map(\.key) }
    func writes() -> [[String]] { t.requests.map(\.arguments).filter { $0.contains("reorder") } }
    /// 等写回账号表、读回 state
    func settled(_ want: [String]) async -> Bool { await waitUntil { table().prefix(want.count).elementsEqual(want) && m.arrangement.order.isEmpty } }
    func account(_ k: String) -> Account { m.account(AccountRef(tool: .claude, key: k))! }
    let bob = "工作(bob@example.com)", erin = "erin@example.com", grace = "grace@example.com", carol = "carol@example.com", dave = "测试(dave@example.com)"
    check("官方账号一组，第三方 API 账号一组", m.accountGroups(.claude).map(\.department) == [.claude, .contractors]
          && m.accountGroups(.claude)[1].accounts.map(\.key) == ["直连接口", "香港接口"])
    check("没有第三方 API 账号的工具只有一组", m.accountGroups(.codex).map(\.department) == [.chatgpt])
    check("没动过：照账号表的顺序", official() == [bob, carol, dave, erin, grace])

    m.moveAccounts(.claude, shown: m.accountGroups(.claude)[0].accounts, from: [0], to: 3)
    check("上下拖动：界面马上变，不等 cca", official() == [carol, dave, bob, erin, grace], "\(official())")
    let dragged = await settled([carol, dave, bob, erin, grace])
    check("账号表跟着改：官方账号在前，第三方 API 账号在后", dragged && writes().last == ["reorder", carol, dave, bob, erin, grace, "直连接口", "香港接口"], "\(writes())")
    check("写好之后 App 不再另记一份顺序", !(prefs.string(forKey: "accounts.arrangement") ?? "").contains("carol-example"))
    check("菜单栏、总览跟着变", m.arrangedAccounts(.claude).prefix(5).map(\.key) == official()
          && m.office(now: 0).rooms[0].people.map(\.account.key) == official())
    check("连着拖时不是后台低优先级（用户在等）", t.requests.last { $0.arguments.contains("reorder") }?.background == false)

    m.setOnTop(account(erin), true)
    check("置顶的排最前面", official().first == erin && m.isOnTop(account(erin)))
    let pinned = await settled([erin, carol, dave, bob, grace])
    check("置顶也写回账号表（终端里的顺序和界面一样）", pinned && writes().last?.prefix(3).elementsEqual(["reorder", erin, carol]) == true, "\(writes())")
    let saved = prefs.string(forKey: "accounts.arrangement") ?? ""
    check("置顶记在 App 的设置里，按账号目录名认", saved.contains("claude:erin-example-com") && !saved.contains("erin@example.com"), saved)
    let before = writes().count
    m.moveAccounts(.claude, shown: m.accountGroups(.claude)[0].accounts, from: [0], to: 5)
    try? await Task.sleep(nanoseconds: 80_000_000)
    check("置顶的只在置顶的里面挪：顺序没变就不写", official().first == erin && writes().count == before)
    m.moveAccounts(.claude, shown: m.accountGroups(.claude)[0].accounts, from: [4], to: 0)
    check("没置顶的拖到最前面：停在置顶的后面", official() == [erin, grace, carol, dave, bob], "\(official())")
    _ = await settled([erin, grace, carol, dave, bob])
    check("拖到同一区的卡片上：排到那张的位置", m.dropAccount(account(bob).stableID, onto: account(grace)) && official() == [erin, bob, grace, carol, dave], "\(official())")
    _ = await settled([erin, bob, grace, carol, dave])
    check("往后拖到卡片上：排到那张的位置", m.dropAccount(account(bob).stableID, onto: account(carol)) && official() == [erin, grace, carol, bob, dave], "\(official())")
    _ = await settled([erin, grace, carol, bob, dave])
    check("跨区拖不过去", !m.dropAccount(account(grace).stableID, onto: account(erin)) && official().first == erin)
    let api = m.accountGroups(.claude)[1].accounts[0]
    check("跨组拖不过去", !m.dropAccount(api.stableID, onto: account(grace)))
    check("认不出的东西拖不进来", !m.dropAccount("随便一段文字", onto: account(grace)))

    let reopened = makeModel(t, prefs: prefs)
    await reopened.start()
    check("下次打开还是这样", reopened.accountGroups(.claude)[0].accounts.map(\.key) == official() && reopened.isOnTop(reopened.account(AccountRef(tool: .claude, key: erin))!))
    reopened.shutdown()

    box.value = box.value.replacingOccurrences(of: "工作(bob@example.com)", with: "新名字(bob@example.com)")
    await m.loadState()
    let renamed = official()
    check("改了别名：位置不变", renamed == [erin, grace, carol, "新名字(bob@example.com)", dave], "\(renamed)")
    box.value = reordered(box.value, tool: "claude", keys: [dave, carol])
    await m.loadState()
    check("在终端里改了顺序：App 跟着变（置顶的还在最前面）", official().prefix(3).elementsEqual([erin, dave, carol]), "\(official())")
    m.shutdown()

    func scripted(_ hello: String, reorderExit: Int32 = 0, prefs: MemoryPrefs = MemoryPrefs()) async -> (ScriptedTransport, AppModel) {
        let b = Box(state)
        let tr = ScriptedTransport { request, _ in
            if request.arguments == ["_gui", "hello"] { return response(0, stdout: hello) }
            if request.arguments == ["_gui", "state"] { return response(0, stdout: b.value) }
            if let i = request.arguments.firstIndex(of: "reorder") {
                if reorderExit != 0 { return response(reorderExit, stderr: "写不了") }
                b.value = reordered(b.value, tool: i == 0 ? "claude" : "codex", keys: Array(request.arguments[(i + 1)...]))
                return response(0)
            }
            return response(0, stdout: SampleDocs.clientUpdates)
        }
        let model = makeModel(tr, prefs: prefs)
        await model.start()
        return (tr, model)
    }
    let (ft, fm) = await scripted(hello, reorderExit: 1)
    fm.moveAccounts(.claude, shown: fm.accountGroups(.claude)[0].accounts, from: [0], to: 3)
    _ = await waitUntil { fm.arrangement.order.isEmpty && fm.toast != nil }
    check("写不进账号表：说一声，顺序回到账号表的样子", fm.toast?.text.contains("没能把顺序写进账号表") == true
          && fm.accountGroups(.claude)[0].accounts.first?.key == bob && ft.requests.contains { $0.arguments.contains("reorder") }, fm.toast?.text ?? "")
    fm.shutdown()

    let oldHello = hello.replacingOccurrences(of: "\"reorder\": 1,", with: "")
    let (ot, om) = await scripted(oldHello)
    om.moveAccounts(.claude, shown: om.accountGroups(.claude)[0].accounts, from: [0], to: 3)
    try? await Task.sleep(nanoseconds: 80_000_000)
    check("旧版 cca 不支持写回：不运行，顺序留在 App 里", !ot.requests.contains { $0.arguments.contains("reorder") }
          && om.accountGroups(.claude)[0].accounts.first?.key == carol && !om.arrangement.order.isEmpty)
    om.shutdown()

    let oldPrefs = MemoryPrefs()
    oldPrefs.set(#"{"order":{"claude":["claude:grace-example-com-0d1e2f","claude:bob-example-com-1a2b3c"]},"top":[]}"#, forKey: "accounts.arrangement")
    let (mt, mm) = await scripted(hello, prefs: oldPrefs)
    let migrated = await waitUntil { mm.arrangement.order.isEmpty && mm.accounts(.claude).first?.key == grace }
    check("上一版只记在 App 里的顺序：第一次读到账号表时写回去", migrated && mt.requests.filter { $0.arguments.contains("reorder") }.count == 1)
    mm.shutdown()

    check("添加第三方 API 账号：带上这个工具的前缀", Handoff.addAPI(prefix: ["codex"]).arguments == ["codex", "add", "api"] && Handoff.addAPI().arguments == ["add", "api"]
          && Handoff.addAPI().inApp)
}

@MainActor func exitPollingTests() async {
    section("自动检测网络出口")
    let now = Date().timeIntervalSince1970
    func item(_ key: String, inUse: Bool = false, age: Double) -> UsagePolling.Item { UsagePolling.Item(key: key, inUse: inUse, updated: now - age) }
    func due(_ items: [UsagePolling.Item], visible: Bool, lowPower: Bool = false) -> [String] {
        ExitPolling.dueKeys(items, visible: visible, lowPower: lowPower, now: now)
    }
    check("看得见：在用的半小时一次", due([item("a", inUse: true, age: 1799), item("b", inUse: true, age: 1801)], visible: true) == ["b"])
    check("看得见：没在用的两小时一次", due([item("a", age: 7199), item("b", age: 7201)], visible: true) == ["b"])
    check("看不见：在用的一小时一次", due([item("a", inUse: true, age: 3599), item("b", inUse: true, age: 3601)], visible: false) == ["b"])
    check("看不见：没在用的四小时一次（比 cca 算过时的 6 小时短）", due([item("a", age: 14399), item("b", age: 14401)], visible: false) == ["b"] && 14400 < 6 * 3600)
    check("低电量模式慢一倍", due([item("a", inUse: true, age: 3599), item("b", inUse: true, age: 3601)], visible: true, lowPower: true) == ["b"])
    var slower = true
    for inUse in [true, false] {
        for visible in [true, false] {
            slower = slower && ExitPolling.interval(inUse: inUse, visible: visible, lowPower: false) > UsagePolling.interval(inUse: inUse, visible: visible, lowPower: false)
        }
    }
    check("都比查用量慢", slower)
    let many = (0..<9).map { item("k\($0)", age: 100_000 + Double($0)) }
    check("一次最多测 6 个，最久没测的先测", due(many, visible: true) == ["k8", "k7", "k6", "k5", "k4", "k3"])

    let poll = CcaCommand.checkPoll(.claude, keys: ["a", "工作(b@example.com)"])
    check("只测这几个账号，低优先级", poll.arguments == ["check", "a", "工作(b@example.com)", "--json"] && poll.isBackground && poll.tool == .claude)
    check("ChatGPT 账号带前缀", CcaCommand.checkPoll(.codex, keys: ["x"]).arguments == ["codex", "check", "x", "--json"])
    check("用户点的不降优先级", !CcaCommand.check(.claude, key: "a").isBackground)

    guard let hello = fixtureText("hello.json"), let state = fixtureText("state.json") else {
        check("夹具", false)
        return
    }
    let t = ScriptedTransport(delay: 0.05) { request, _ in
        if request.arguments == ["_gui", "hello"] { return response(0, stdout: hello) }
        if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
        if request.arguments.contains("check") { return response(1, stderr: "测不了") }
        return response(1, stderr: "查不到")
    }
    let log = HookLog()
    let m = makeModel(t, log: log, prefs: MemoryPrefs())
    await m.start()
    func checks() -> [CcaRequest] { t.requests.filter { $0.arguments.contains("check") && $0.arguments.first != "clients" } }
    func states() -> Int { t.requests.filter { $0.arguments == ["_gui", "state"] }.count }
    log.conditions.idleSeconds = UsagePolling.awayAfter + 1
    m.refreshUsageIfNeeded()
    check("用户走开了不测", checks().isEmpty)
    log.conditions = SystemConditions(thermalPressure: true)
    m.refreshUsageIfNeeded()
    check("系统过热不测", checks().isEmpty)
    log.conditions = SystemConditions()
    m.refreshUsageIfNeeded()
    check("正在查用量时等下一分钟", checks().isEmpty && !m.usageRefreshingKeys.isEmpty)
    _ = await waitUntil { m.usageRefreshingKeys.isEmpty }
    let before = states()
    m.refreshUsageIfNeeded()
    let busy = m.activities.isEmpty
    _ = await waitUntil { t.requests.filter { $0.arguments.contains("check") }.count >= 2 && states() > before }
    check("不占用账号的按钮", busy && m.activities.isEmpty)
    let args = checks().map(\.arguments)
    check("到期的账号测了：登录着的和第三方 API 账号，没登录、登错号的不测",
          args.contains { $0.contains("工作(bob@example.com)") && $0.contains("直连接口") && !$0.contains("carol@example.com") && !$0.contains("测试(dave@example.com)") },
          args.map { $0.joined(separator: " ") }.joined(separator: " | "))
    check("ChatGPT 的账号一起测", args.contains { $0.prefix(2) == ["codex", "check"] && $0.contains("frank@example.com") })
    check("低优先级运行", checks().allSatisfy(\.background))
    check("测完读一次 state（结果在 health 里）", states() > before)
    let count = checks().count
    m.refreshUsageIfNeeded()
    try? await Task.sleep(nanoseconds: 120_000_000)
    check("没测成不马上重试（按间隔）", checks().count == count)
    m.shutdown()
}

@MainActor func reloginTests() async {
    section("登录失效，重新登录")
    let now = Date().timeIntervalSince1970
    func item(_ age: Double, _ login: UsagePolling.LoginWatch) -> UsagePolling.Item { UsagePolling.Item(key: "a", inUse: true, updated: now - age, login: login) }
    func due(_ i: UsagePolling.Item, visible: Bool) -> Bool { !UsagePolling.dueKeys([i], visible: visible, lowPower: false, now: now).isEmpty }
    check("登录失效的看得见时 15 分钟查一次（多查没用）", !due(item(899, .expired), visible: true) && due(item(901, .expired), visible: true))
    check("看不见时一小时", !due(item(3599, .expired), visible: false) && due(item(3601, .expired), visible: false))
    check("点了重新登录之后每分钟查一次，看不见时也是", !due(item(50, .relogging), visible: false) && due(item(56, .relogging), visible: false))

    guard let hello = fixtureText("hello.json"), let state = fixtureText("state.json") else {
        check("夹具", false)
        return
    }
    let frank = "frank@example.com"
    // cca 查不到时用缓存顶上：fetched_epoch 就是缓存的时间
    func revoked(_ fetched: Double) -> String {
        """
        {"version":1,"kind":"usage","tool":"codex","generated_epoch":\(now),"accounts":[{"account":"\(frank)","key":"\(frank)","email":"\(frank)","plan":"Pro","source":"cache","fetched_epoch":\(fetched),"stale":false,"error":"登录已失效（比如改过密码）：cca codex login \(frank) 重新登录","error_code":"auth","error_detail":"Your access token could not be refreshed because your refresh token was revoked. Please log out and sign in again."}]}
        """
    }
    let box = Box("")
    let t = ScriptedTransport { request, _ in
        if request.arguments == ["_gui", "hello"] { return response(0, stdout: hello) }
        if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
        if request.arguments.contains("usage") { return response(0, stdout: box.value) }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    let log = HookLog()
    let m = makeModel(t, log: log, prefs: MemoryPrefs())
    await m.start()
    let ref = AccountRef(tool: .codex, key: frank)
    guard let a = m.account(ref) else {
        check("夹具里有 frank", false)
        return
    }
    check("查用量之前看着是好的", a.login == .loggedIn && !m.loginExpired(a))
    let cachedAt = a.usage?.fetchedEpoch ?? 0
    box.value = revoked(cachedAt)
    m.refreshUsage(tool: .codex, key: frank, manual: false)
    _ = await waitUntil { m.usageRefreshingKeys.isEmpty && m.usage(for: a)?.errorCode == .auth }
    check("查用量时知道登录失效了", m.loginExpired(a))
    let diagnosis = Diagnosis.all(a, usage: m.usage(for: a), live: nil, now: now)
    check("要紧的事只说这一件", diagnosis == [.loginExpired])
    let e = Employee.make(a, usage: m.usage(for: a), live: nil, now: now)
    check("总览里算要处理的", e.mood == .expired && OfficeCensus([e]).needsAttention == 1 && Texts.officeSummary(OfficeCensus([e])).contains("1 个登录失效"))
    check("文字", Texts.mood(e, appLabel: "ChatGPT App", now: now, brief: true) == "登录已失效")
    let h = m.loginHandoff(a)
    check("重新登录的命令", h.arguments == ["codex", "login", frank] && h.title.hasPrefix("重新登录") && h.message.contains("退出"))
    m.relogin(a)
    check("登录失效的不用再确认，直接去终端", m.sheet == nil)
    m.dismissSheet()

    let bob = m.account(AccountRef(tool: .claude, key: "工作(bob@example.com)"))!
    m.relogin(bob)
    if case .handoff(let d)? = m.sheet {
        check("登录还好好的账号先问一句（会退掉现在的登录）", d.arguments == ["login", "工作(bob@example.com)"] && d.message.contains("退出"))
    } else {
        check("登录还好好的账号先问一句（会退掉现在的登录）", false)
    }
    m.dismissSheet()

    let carol = m.account(AccountRef(tool: .claude, key: "carol@example.com"))!
    check("没登录的还是“登录”", m.loginHandoff(carol).title.hasPrefix("登录 "))

    box.value = revoked(now).replacingOccurrences(of: "\"error_code\":\"auth\"", with: "\"error_code\":null").replacingOccurrences(of: "\"source\":\"cache\"", with: "\"source\":\"live\"")
    m.refreshUsage(tool: .codex, key: frank, manual: false)
    _ = await waitUntil { m.usageRefreshingKeys.isEmpty && m.usage(for: a)?.errorCode == nil }
    check("重新登录好了：提示就没了", !m.loginExpired(a))
    m.shutdown()
}
