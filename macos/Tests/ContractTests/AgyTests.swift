import Foundation

// Antigravity（agy）：第三种工具。hello 的 tools.agy、state 的第三个工具和它的 features（没有桌面 App、第三方 API 账号：can_why 是
// unsupported；用量是几组额度 quota_groups，3.6.10 及以前的 cca 给的 features.usage 是 false），总览多一组、用量统计页不算它，
// 没启用时绝不调用 cca agy …；额度怎么算用完、小条写哪个、查得多勤

private func text(_ name: String) -> String? { fixture(name).map { String(decoding: $0, as: UTF8.self) } }

func agyFixtureTests() {
    section("Antigravity：数据")
    let gina = AccountRef(tool: .agy, key: "工作(gina@example.com)"), henry = AccountRef(tool: .agy, key: "henry@example.com")
    if let h = loadFixture(HelloDoc.self, "hello_agy.json") {
        check("hello_agy：启用了 Antigravity、没启用 Codex", h.enabled(.agy) && !h.codexEnabled && h.enabledTools == [.claude, .agy],
              "\(h.enabledTools)")
        check("hello_agy：agy 命令的路径", h.tools[.agy]?.bin == "/Users/me/.local/bin/agy")
    }
    if let h = loadFixture(HelloDoc.self, "hello.json") {
        check("旧版 cca 的 hello 没有 tools.agy：当作没启用", !h.enabled(.agy) && h.enabledTools == [.claude, .codex])
    }
    guard let s = loadFixture(StateDoc.self, "state_agy.json") else { return }
    let ag = s.tool(.agy)
    check("state_agy：第三个工具是 agy，命令前缀 [agy]", s.tools.map(\.id) == [.claude, .codex, .agy] && ag?.cliPrefix == ["agy"]
          && ag?.enabled == true && ag?.accounts.count == 2)
    check("state_agy：features（agy 只有用量，Claude 都有）", ag?.features == ToolFeatures(app: false, api: false, usage: true)
          && s.tool(.claude)?.features == ToolFeatures(app: true, api: true, usage: true))
    let g = s.account(gina), hy = s.account(henry)
    check("gina：登录着、Google AI Pro / standard、命令行开着", g?.login == .loggedIn && g?.plan == "Google AI Pro" && g?.planTier == .standard
          && g?.cliSessions == 1 && g?.isWorking == true && g?.port == 7902)
    check("gina：没有桌面 App（unsupported），用量、网页版、出口检测、终端都能用",
          g?.can.app == false && g?.can.usage == true && g?.canWhy["app"] == .unsupported && g?.canWhy["usage"] == nil
          && g?.can.web == true && g?.can.check == true && g?.can.cli == true && g?.can.billing == false && g?.can.limitResets == false)
    check("henry：没登录，查不了用量", hy?.login == .loggedOut && hy?.canWhy["usage"] == .notLoggedIn)
    check("gina 的用量是缓存里的几组额度", g?.usage?.source == .cache && g?.usage?.hasQuota == true
          && g?.usage?.quotaGroups.map(\.name) == ["Gemini Pro", "Claude", "Gemini Flash"] && hy?.usage == nil)
    check("槽位、节点的使用者里有 agy 的账号", s.routing?.slots.first { $0.port == 7902 }?.tool == .agy
          && s.routing?.nodes.first { $0.name == "US-B" }?.users.contains { $0.tool == .agy && $0.key == gina.key } == true)
    check("unsupported 的说法", Texts.canWhy(.unsupported, appLabel: "桌面 App", tool: .agy) == "Antigravity 账号没有桌面 App"
          && Texts.canWhy(.unsupported, appLabel: "Claude App", tool: .claude) == "这种账号没有这个功能")
    check("没装 agy 命令", Texts.canWhy(.toolMissing, appLabel: "", tool: .agy) == "没装 agy 命令"
          && Texts.canWhy(.toolMissing, appLabel: "", tool: .codex) == "没装 codex 命令"
          && Texts.canWhy(.toolMissing, appLabel: "", tool: .claude) == "没装 claude 命令")
    check("叫法", Texts.product(.agy) == "Antigravity" && Texts.department(.antigravity) == "Antigravity 账号"
          && Texts.cliName(.agy) == "agy" && Texts.cliName(.codex) == "Codex" && Texts.cliName(.claude) == "Claude Code")
    check("旧版 cca（features.usage 是 false）：用量那一节说更新 cca，或者在终端里用 cca agy usage 看", Texts.noUsage(.agy).contains("cca agy usage")
          && Texts.noUsage(.agy).contains("更新 cca"))

    // cca agy usage --json：额度在 quota_groups 里
    if let u = loadFixture(UsageDoc.self, "usage_agy.json") {
        let a = u.accounts.first, h = u.accounts.last, groups = u.accounts.first?.quotaGroups ?? []
        check("usage_agy：tool 是 agy；没有 5 小时、本周这些窗口（hasData 是 false）", u.tool == .agy && a?.source == .live
              && a?.fiveHour == nil && a?.sevenDay == nil && a?.hasData == false)
        check("usage_agy：三组额度和 agy 的说明", groups.map(\.name) == ["Gemini Pro", "Claude", "Gemini Flash"]
              && groups.map(\.buckets.count) == [2, 2, 1] && a?.quotaDescription == "Quota for Antigravity models", "\(groups.map(\.name))")
        let pro = groups.first?.buckets.first, requests = groups.count > 1 ? groups[1].buckets.last : nil, flash = groups.last?.buckets.first
        check("usage_agy：用了百分之几、什么时候重置是 cca 算好的", pro?.used.utilization == 13 && pro?.used.resetsEpoch == 1794051000
              && pro?.used.reset == false && pro?.window == "5h" && pro?.id == "gemini-pro-5h" && pro?.disabled == false)
        check("usage_agy：cca 认出的窗口（slot）", groups.first?.buckets.map(\.slot) == [.fiveHour, .sevenDay]
              && groups.count > 1 && groups[1].buckets.map(\.slot) == [.fiveHour, nil])
        if let a {
            let now = 1794042000.0
            let m5 = a.quotaMost(.fiveHour, now: now), mw = a.quotaMost(.sevenDay, now: now)
            check("小条：各组里用得最多的那个（5 小时是 Claude 的 95%，本周只有 Gemini Pro 有）", m5?.window.utilization == 95 && m5?.group == "Claude"
                  && mw?.window.utilization == 75 && mw?.group == "Gemini Pro")
            check("用完没用完看用得最少的那个：5 小时 Gemini Pro 才 13%，不紧张", a.quotaLeast(.fiveHour, now: now)?.utilization == 13
                  && Diagnosis.quotaPressure(a, now: now).isEmpty)
            check("没有百分比的、停用的不算", a.quotaBuckets(.fiveHour).count == 2 && a.quotaBuckets(.sevenDay).count == 1)
        }
        check("usage_agy：没有百分比的额度带着还剩几次；停用的额度", requests?.remainingAmount == 12 && requests?.used.utilization == nil
              && requests?.id == nil && flash?.disabled == true && flash?.used.utilization == nil)
        check("usage_agy：没登录的账号没有额度", h?.source == UsageSource.none && h?.errorCode == .login && h?.quotaGroups.isEmpty == true)
        // 界面上的组名不放没翻译的英文
        do {
            let j = #"[{"name": "Gemini Models", "buckets": []}, {"name": "Claude and GPT models", "buckets": []}, {"name": "Gemini Flash", "buckets": []}, {"name": " ", "buckets": []}]"#
            let gs = try JSONDecoder().decode([QuotaGroup].self, from: Data(j.utf8))
            check("组名：… models 写成 … 模型、and 写成顿号，模型名照写，没有名字叫额度",
                  gs.map(\.displayName) == ["Gemini 模型", "Claude、GPT 模型", "Gemini Flash", "额度"])
        } catch {
            check("组名的样例解码：\(error)", false)
        }
    }
    quotaPressureTests()
    check("没有桌面 App：不给下载 App 的补救", Remedy.forCode(.appMissing, tool: .agy, key: gina.key) == Remedy.none
          && Remedy.forCode(.appMissing, tool: .claude, key: nil) != Remedy.none)

    // 旧版 cca 的 state 没有 features：Antigravity 什么都没有，别的工具都有
    let noFeatures = #"{"id":"agy","cli_prefix":["agy"],"enabled":true,"accounts":[]}"#
    let old = try? JSONDecoder().decode(ToolState.self, from: Data(noFeatures.utf8))
    let oldCodex = try? JSONDecoder().decode(ToolState.self, from: Data(#"{"id":"codex","enabled":true,"accounts":[]}"#.utf8))
    check("没给 features：按工具认", old?.features == ToolFeatures.defaults(.agy) && old?.features.usage == false
          && oldCodex?.features == ToolFeatures(app: true, api: true, usage: true) && oldCodex?.cliPrefix == ["codex"])
    let partial = try? JSONDecoder().decode(ToolFeatures.self, from: Data(#"{"usage":false}"#.utf8))
    check("features 少了几项：没写的当作有", partial == ToolFeatures(app: true, api: true, usage: false))

    // 总览：Antigravity 一组排在 Claude（和 ChatGPT）后面、第三方 API 前面；没启用就没有这一组
    let o = Office.make(tools: s.tools, enabled: [.claude, .agy], usage: { $0.usage }, live: { _ in nil }, now: 1794042000)
    check("总览：Claude、Antigravity、第三方 API 三组", o.rooms.map(\.department) == [.claude, .antigravity, .contractors],
          "\(o.rooms.map(\.department))")
    let moods = Dictionary(uniqueKeysWithValues: o.people.map { ($0.account.key, $0.mood) })
    check("总览：gina 在用（命令行开着）、henry 没登录", moods[gina.key] == .working && moods[henry.key] == .absent, "\(moods)")
    check("总览：没启用 agy 就没有这一组", Office.make(tools: s.tools, enabled: [.claude], usage: { $0.usage }, live: { _ in nil },
                                                now: 1794042000).rooms.map(\.department) == [.claude, .contractors])
    check("组和工具一一对应", Employee.Department.official(.agy) == .antigravity && Employee.Department.antigravity.tool == .agy
          && Employee.Department.antigravity.key == "agy")
    check("添加 Antigravity 账号：cca agy add", Employee.Department.antigravity.addHandoff(prefix: ["agy"]).arguments == ["agy", "add"]
          && Employee.Department.antigravity.addHandoff(prefix: ["agy"]).title == "添加 Antigravity 账号")
    check("编辑账号表的标题按工具说", Handoff.edit(prefix: ["agy"]).title == "编辑 Antigravity 账号表"
          && Handoff.edit(prefix: ["codex"]).title == "编辑 ChatGPT 账号表" && Handoff.edit(prefix: []).title == "编辑 Claude 账号表")
}

func agyGateTests() async {
    section("Antigravity：闸门")
    let t = ScriptedTransport { _, _ in response(0) }
    let c = CcaClient(transport: t)
    await c.setEnabledTools([.claude, .codex])
    let r = await c.execute(.check(.agy, key: "gina@example.com"), prompter: nil)
    expectFailure("没启用 Antigravity 时不运行 cca agy …", r.status) {
        $0.reason == .codexDisabled && $0.message == "还没启用 Antigravity 账号" && t.requests.isEmpty
    }
    await c.setEnabledTools([.agy])
    _ = await c.execute(.check(.agy, key: "gina@example.com"), prompter: nil)
    _ = await c.execute(.web(.agy, key: "gina@example.com"), prompter: nil)
    let r2 = await c.execute(.check(.codex, key: "frank@example.com"), prompter: nil)
    check("启用了：参数带 agy", t.requests.map(\.arguments) == [["agy", "check", "gina@example.com", "--json"], ["agy", "web", "gina@example.com"]],
          "\(t.requests.map(\.arguments))")
    expectFailure("只启用 agy 时 Codex 照样不运行（Claude 一直可以）", r2.status) { $0.reason == .codexDisabled }
}

@MainActor func agyModelTests() async {
    section("Antigravity：App")
    guard let hello = text("hello_agy.json"), let state = text("state_agy.json"), let oldState = text("state_agy_old.json") else {
        check("夹具", false)
        return
    }
    let t = ScriptedTransport { request, _ in
        if request.arguments == ["_gui", "hello"] { return response(0, stdout: hello) }
        if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
        if request.arguments.contains("usage") { return response(0, stdout: tinyUsage) }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    let m = makeModel(t, prefs: MemoryPrefs())
    await m.start()
    check("看得见的工具：Claude、Antigravity", m.visibleTools == [.claude, .agy], "\(m.visibleTools)")
    check("features 取 cca 给的", m.features(.agy) == ToolFeatures(app: false, api: false, usage: true) && m.features(.claude).usage)
    check("命令前缀、App 的叫法", m.prefix(.agy) == ["agy"] && m.appLabel(.agy) == "桌面 App")
    check("用量统计页只有 Claude 一组（agy 没有每周用量的统计）", m.officialGroups().map(\.department) == [.claude],
          "\(m.officialGroups().map(\.department))")
    check("侧栏：agy 的官方账号一组", m.accountGroups(.agy).map(\.department) == [.antigravity]
          && m.accountGroups(.agy).first?.accounts.map(\.key) == ["工作(gina@example.com)", "henry@example.com"])
    check("总览：三组", m.office(now: 1794042000).rooms.map(\.department) == [.claude, .antigravity, .contractors])
    check("添加：cca agy add", m.addHandoff(.antigravity).arguments == ["agy", "add"])
    m.tool = .agy
    m.refreshUsage(manual: true)
    m.refreshUsage(tool: .agy, key: "工作(gina@example.com)", manual: true)
    _ = await waitUntil(1) { !m.isUsageRefreshing(AccountRef(tool: .claude, key: "alice@example.com")) }
    let agyUsage = t.requests.filter { $0.arguments.first == "agy" && $0.arguments.contains("usage") }
    check("查 agy 的用量：cca agy usage 账号 --json", agyUsage.contains { $0.arguments == ["agy", "usage", "工作(gina@example.com)", "--json"] },
          "\(agyUsage.map(\.arguments))")
    check("Claude 的照常查", t.requests.contains { $0.arguments == ["usage", "--json"] })
    m.shutdown()

    // 3.6.10 及以前的 cca（state_agy_old.json：agy 的 features.usage 是 false，can_why.usage 是 unsupported）：
    // 点了“登录”、等着登录好的那段时间（UsagePolling.reloginWatch），后台不跑 cca agy usage（旧版 cca 只会失败，留下一条“后台刷新 Antigravity 的用量没有成功”；新版的额度 App 也不显示），按同样的间隔只重读 state，登录好了界面照样会变
    let unsupported = EventLog.parseLine(Data(#"{"v":1,"ev":"fail","code":"unsupported","message":"Antigravity 没有能在 agy 外面查的用量接口：在 agy 里用 /usage 看"}"#.utf8))!
    let t3 = ScriptedTransport { request, _ in
        if request.arguments == ["_gui", "hello"] { return response(0, stdout: hello) }
        if request.arguments == ["_gui", "state"] { return response(0, stdout: oldState) }
        if request.arguments.first == "agy", request.arguments.contains("usage") { return response(1, [unsupported]) }
        if request.arguments.contains("usage") { return response(0, stdout: tinyUsage) }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    let m3 = makeModel(t3, prefs: MemoryPrefs())
    await m3.start()
    func states() -> Int { t3.requests.filter { $0.arguments == ["_gui", "state"] }.count }
    // 先把到期的用量、出口检测都跑完（查用量时同一个工具的出口检测等下一轮），之后多出来的 state 只能是盯着登录的那一次
    for _ in 0..<3 {
        m3.refreshUsageIfNeeded()
        _ = await waitUntil(1) { m3.usageRefreshingKeys.isEmpty }
        try? await Task.sleep(nanoseconds: 150_000_000)
    }
    let henry = m3.account(AccountRef(tool: .agy, key: "henry@example.com"))
    check("夹具里 henry 没登录", henry?.login == .loggedOut)
    if let henry { m3.relogin(henry) }
    let before = states()
    m3.refreshUsageIfNeeded()
    _ = await waitUntil(1) { states() > before }
    m3.refreshUsageIfNeeded()
    try? await Task.sleep(nanoseconds: 150_000_000)
    let agyPolls = t3.requests.filter { $0.arguments.first == "agy" && $0.arguments.contains("usage") }
    check("点了登录以后：后台不跑 cca agy usage", agyPolls.isEmpty, "\(agyPolls.map(\.arguments))")
    check("所以没有“后台刷新 Antigravity 的用量没有成功”", m3.usageFailure[.agy] == nil, m3.usageFailure[.agy]?.message ?? "")
    check("改成重读 state 看登录好了没有，按间隔（马上再来一次不重读）", states() == before + 1, "多读了 \(states() - before) 次")
    m3.refreshUsage(tool: .agy, key: "工作(gina@example.com)", manual: true)
    try? await Task.sleep(nanoseconds: 150_000_000)
    check("旧版 cca：点了刷新也不跑 cca agy usage", !t3.requests.contains { $0.arguments.first == "agy" && $0.arguments.contains("usage") })
    m3.shutdown()

    // 没启用 agy 的 hello：工具换回 Claude，看不见 agy
    let off = hello.replacingOccurrences(of: #""enabled": true,\#n      "bin": "/Users/me/.local/bin/agy""#,
                                         with: #""enabled": false,\#n      "bin": "/Users/me/.local/bin/agy""#)
    check("改出没启用 agy 的 hello", off != hello)
    let t2 = ScriptedTransport { request, _ in
        if request.arguments == ["_gui", "hello"] { return response(0, stdout: off) }
        if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    let m2 = makeModel(t2, prefs: MemoryPrefs())
    await m2.start()
    check("没启用：只有 Claude", m2.visibleTools == [.claude] && m2.office(now: 1794042000).rooms.map(\.department) == [.claude, .contractors])
    check("没启用：一条 cca agy … 都不跑", !t2.requests.contains { $0.arguments.first == "agy" })
    m2.shutdown()
}

/// Antigravity 的额度什么时候算用完：每一组的这个窗口都用完了才算（一组用完了，别的组的模型照样能用），到最早重置的那一组时就能接着用；
/// 过了重置时间的算 0。菜单栏图标的标记也照这个。
private func quotaPressureTests() {
    func entry(_ buckets: [(Double?, Double, Bool)]) -> UsageEntry? {
        let groups = buckets.enumerated().map { i, b in
            #"{"name": "G\#(i)", "buckets": [{"window": "5h", "slot": "five_hour", "disabled": false, "utilization": \#(b.0.map { "\($0)" } ?? "null"), "resets_epoch": \#(b.1), "reset": \#(b.2)}]}"#
        }
        let json = #"{"account": "a", "key": "a@example.com", "source": "live", "fetched_epoch": 1794041990, "stale": false, "quota_groups": [\#(groups.joined(separator: ","))]}"#
        return try? JSONDecoder().decode(UsageEntry.self, from: Data(json.utf8))
    }
    let now = 1794042000.0
    if let u = entry([(100, 1794051000, false), (100, 1794046320, false)]) {
        let p = Diagnosis.quotaPressure(u, now: now)
        check("两组的 5 小时都用完了：算用完，最早重置的那组什么时候重置就什么时候能接着用",
              p.count == 1 && p.first?.window == .fiveHour && p.first?.exhausted == true && p.first?.resetsEpoch == 1794046320, "\(p)")
        check("菜单栏图标带标记", Attention.needed(sync: nil, live: nil, usages: [u], now: now))
    } else { check("拼出额度", false) }
    if let u = entry([(100, 1794051000, false), (40, 1794046320, false)]) {
        check("只有一组用完了：不算（另一组的模型照样能用），小条写用完的那组", Diagnosis.quotaPressure(u, now: now).isEmpty
              && u.quotaMost(.fiveHour, now: now)?.window.utilization == 100 && !Attention.needed(sync: nil, live: nil, usages: [u], now: now))
    } else { check("拼出额度", false) }
    if let u = entry([(100, 1794051000, false), (100, 1794040000, true)]) {
        check("用完的那组过了重置时间：算 0，不算用完", Diagnosis.quotaPressure(u, now: now).isEmpty)
    } else { check("拼出额度", false) }
    if let u = entry([(95, 1794051000, false), (92, 1794046320, false)]) {
        let p = Diagnosis.quotaPressure(u, now: now)
        check("每组都快用完了（≥90%）：快用完，百分比是用得最少的那组的", p.count == 1 && p.first?.exhausted == false && p.first?.percent == 92, "\(p)")
    } else { check("拼出额度", false) }
    check("查得慢：看得见时在用的 10 分钟、没在用的 20 分钟，看不见时 20 分钟、1 小时；登录失效、在等登录的和别的工具一样",
          UsagePolling.interval(inUse: true, visible: true, lowPower: false, slow: true) == 600
          && UsagePolling.interval(inUse: false, visible: true, lowPower: false, slow: true) == 1200
          && UsagePolling.interval(inUse: true, visible: false, lowPower: false, slow: true) == 1200
          && UsagePolling.interval(inUse: false, visible: false, lowPower: false, slow: true) == 3600
          && UsagePolling.interval(inUse: false, visible: true, lowPower: true, slow: true) == 2400
          && UsagePolling.interval(inUse: false, visible: true, lowPower: false, login: .relogging, slow: true) == 55
          && UsagePolling.interval(inUse: false, visible: true, lowPower: false) == 480)
}
