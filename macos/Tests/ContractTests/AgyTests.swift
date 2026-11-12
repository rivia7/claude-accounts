import Foundation

// Antigravity（agy）：第三种工具。hello 的 tools.agy、state 的第三个工具和它的 features（没有桌面 App、第三方 API 账号、
// 查用量的接口：can_why 是 unsupported），总览多一组、用量统计不算它，没启用时绝不调用 cca agy …

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
    check("state_agy：features（agy 什么都没有，Claude 都有）", ag?.features == ToolFeatures(app: false, api: false, usage: false)
          && s.tool(.claude)?.features == ToolFeatures(app: true, api: true, usage: true))
    let g = s.account(gina), hy = s.account(henry)
    check("gina：登录着、Google AI Pro / standard、命令行开着", g?.login == .loggedIn && g?.plan == "Google AI Pro" && g?.planTier == .standard
          && g?.cliSessions == 1 && g?.isWorking == true && g?.port == 7902)
    check("gina：没有桌面 App、查不了用量（unsupported），网页版、出口检测、终端都能用",
          g?.can.app == false && g?.can.usage == false && g?.canWhy["app"] == .unsupported && g?.canWhy["usage"] == .unsupported
          && g?.can.web == true && g?.can.check == true && g?.can.cli == true && g?.can.billing == false && g?.can.limitResets == false)
    check("henry：没登录也是 unsupported（不说“没登录查不了用量”）", hy?.login == .loggedOut && hy?.canWhy["usage"] == .unsupported)
    check("槽位、节点的使用者里有 agy 的账号", s.routing?.slots.first { $0.port == 7902 }?.tool == .agy
          && s.routing?.nodes.first { $0.name == "US-B" }?.users.contains { $0.tool == .agy && $0.key == gina.key } == true)
    check("unsupported 的说法", Texts.canWhy(.unsupported, appLabel: "桌面 App", tool: .agy) == "Antigravity 账号没有桌面 App 和能查的用量接口"
          && Texts.canWhy(.unsupported, appLabel: "Claude App", tool: .claude) == "这种账号没有这个功能")
    check("没装 agy 命令", Texts.canWhy(.toolMissing, appLabel: "", tool: .agy) == "没装 agy 命令"
          && Texts.canWhy(.toolMissing, appLabel: "", tool: .codex) == "没装 codex 命令"
          && Texts.canWhy(.toolMissing, appLabel: "", tool: .claude) == "没装 claude 命令")
    check("叫法", Texts.product(.agy) == "Antigravity" && Texts.department(.antigravity) == "Antigravity 账号"
          && Texts.cliName(.agy) == "agy" && Texts.cliName(.codex) == "Codex" && Texts.cliName(.claude) == "Claude Code")
    check("用量那一节：说在 agy 里用 /usage 看", Texts.noUsage(.agy).contains("/usage"))
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
    guard let hello = text("hello_agy.json"), let state = text("state_agy.json") else {
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
    check("features 取 cca 给的", m.features(.agy) == ToolFeatures(app: false, api: false, usage: false) && m.features(.claude).usage)
    check("命令前缀、App 的叫法", m.prefix(.agy) == ["agy"] && m.appLabel(.agy) == "桌面 App")
    check("用量统计只有 Claude 一组（agy 没有查用量的接口）", m.officialGroups().map(\.department) == [.claude],
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
    check("不查 agy 的用量（cca agy usage 只会说不支持）", agyUsage.isEmpty, "\(agyUsage.map(\.arguments))")
    check("Claude 的照常查", t.requests.contains { $0.arguments == ["usage", "--json"] })
    m.shutdown()

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
