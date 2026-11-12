import Foundation

// 自动刷新用量、检查客户端更新的节奏：只查到期的账号，系统吃紧、用户走开时不查，后台的活低优先级，不空转。

private typealias ResponseOverride = @Sendable () -> CcaResponse

@MainActor func pollingTests() async {
    section("自动刷新的节奏")
    let now = Date().timeIntervalSince1970
    func item(_ key: String, inUse: Bool, age: Double) -> UsagePolling.Item { UsagePolling.Item(key: key, inUse: inUse, updated: now - age) }
    func due(_ items: [UsagePolling.Item], visible: Bool, lowPower: Bool = false) -> [String] {
        UsagePolling.dueKeys(items, visible: visible, lowPower: lowPower, now: now)
    }
    check("看得见：在用的账号 5 分钟一次", due([item("a", inUse: true, age: 299), item("b", inUse: true, age: 301)], visible: true) == ["b"])
    check("看得见：没在用的 8 分钟一次（比变灰的 10 分钟短）", due([item("a", inUse: false, age: 479), item("b", inUse: false, age: 481)], visible: true) == ["b"])
    check("看不见：在用的 10 分钟一次", due([item("a", inUse: true, age: 599), item("b", inUse: true, age: 601)], visible: false) == ["b"])
    check("看不见：没在用的 1 小时一次", due([item("a", inUse: false, age: 3599), item("b", inUse: false, age: 3601)], visible: false) == ["b"])
    check("低电量模式慢一倍", due([item("a", inUse: true, age: 599), item("b", inUse: true, age: 601)], visible: true, lowPower: true) == ["b"])
    check("没查过的马上查", due([UsagePolling.Item(key: "a", inUse: false, updated: 0)], visible: false) == ["a"])

    let poll = CcaCommand.usagePoll(.claude, keys: ["a", "工作(b@example.com)"])
    check("自动刷新只查这几个账号，低优先级，最多同时查两个", poll.arguments == ["usage", "a", "工作(b@example.com)", "--json"] && poll.isBackground
          && poll.extraEnvironment["CCA_USAGE_JOBS"] == "2" && poll.gate == CcaCommand.usage(.claude, key: nil).gate)
    check("ChatGPT 账号带前缀", CcaCommand.usagePoll(.codex, keys: ["x"]).arguments == ["codex", "usage", "x", "--json"])
    check("用户点的不降优先级", !CcaCommand.usage(.claude, key: nil).isBackground && !CcaCommand.clientCheck(force: true).isBackground && CcaCommand.clientCheck(force: false).isBackground)

    // 低优先级真的传到子进程
    let probe = tmpRoot.appendingPathComponent("qos-probe")
    FileManager.default.createFile(atPath: probe.path, contents: Data("#!/bin/sh\nsleep 0.3 &\nps -o pri= -p $!\n".utf8), attributes: [.posixPermissions: 0o755])
    var spec = ProcessSpec(executable: "/bin/sh", arguments: [probe.path], environment: ["PATH": "/usr/bin:/bin"], timeout: 10)
    func childPriority() async -> Int? {
        (try? await CcaProcess.run(spec)).flatMap { Int($0.stdoutText.trimmingCharacters(in: .whitespacesAndNewlines)) }
    }
    let normal = await childPriority()
    spec.background = true
    let low = await childPriority()
    check("后台的活：子进程的调度优先级也降下来", normal != nil && low != nil && low! < normal!, "\(String(describing: normal)) \(String(describing: low))")

    // 模型：到期才查、走开了不查、查失败不马上重试
    guard let hello = fixture("hello.json").map({ String(decoding: $0, as: UTF8.self) }),
          let state = fixture("state.json").map({ String(decoding: $0, as: UTF8.self) }) else {
        check("夹具", false)
        return
    }
    let t = ScriptedTransport { request, _ in
        if request.arguments == ["_gui", "hello"] { return response(0, stdout: hello) }
        if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
        if request.arguments.contains("usage") { return response(1, stderr: "查不到") }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    let log = HookLog()
    let m = makeModel(t, log: log, prefs: MemoryPrefs())
    await m.start()
    func polls() -> [CcaRequest] { t.requests.filter { $0.arguments.contains("usage") } }
    check("连上就开始轮询（App 隐藏时也是）", m.scheduler.usageRunning && !m.isVisible)
    log.conditions.idleSeconds = UsagePolling.awayAfter + 1
    m.refreshUsageIfNeeded()
    check("用户走开了不查", polls().isEmpty)
    log.conditions = SystemConditions(thermalPressure: true)
    m.refreshUsageIfNeeded()
    check("系统过热不查", polls().isEmpty)
    log.conditions = SystemConditions()
    m.refreshUsageIfNeeded()
    _ = await waitUntil { !polls().isEmpty && m.usageRefreshingKeys.isEmpty }
    let first = polls()
    check("到期的账号查了，第三方 API 账号、没登录的不查", !first.isEmpty && first.allSatisfy { !$0.arguments.contains("香港接口") && !$0.arguments.contains("carol@example.com") && $0.arguments.last == "--json" },
          first.map { $0.arguments.joined(separator: " ") }.joined(separator: " | "))
    check("自动刷新以低优先级运行", first.allSatisfy { $0.background && $0.extraEnvironment["CCA_USAGE_JOBS"] == "2" })
    m.refreshUsageIfNeeded()
    try? await Task.sleep(nanoseconds: 80_000_000)
    check("查失败了不马上重试（按间隔）", polls().count == first.count)
    // 没登录的账号点了“登录”：等它登录好的那段时间照旧查用量（登录好了就查得了；查不了用量的工具只重读 state，见 AgyTests）
    if let carol = m.account(AccountRef(tool: .claude, key: "carol@example.com")) {
        m.relogin(carol)
        m.dismissSheet()
        m.refreshUsageIfNeeded()
        _ = await waitUntil(1) { polls().contains { $0.arguments.contains("carol@example.com") } }
        check("点了登录的没登录账号：照旧查用量，看登录好了没有", polls().contains { $0.arguments.contains("carol@example.com") })
    } else {
        check("夹具里有 carol", false)
    }
    m.shutdown()
    check("退出时停下", !m.scheduler.usageRunning)

    // 客户端更新：睡到 cca 给的时间，不空转
    func clientModel(next: Double) -> (ScriptedTransport, AppModel, Scheduler) {
        let doc = SampleDocs.clientUpdates.replacingOccurrences(of: "\"mode\"", with: "\"next_check_epoch\":\(next),\"mode\"")
        let tiny = ScriptedTransport { request, _ in
            if request.arguments == ["_gui", "hello"] { return response(0, stdout: tinyHello) }
            if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
            return response(0, stdout: doc)
        }
        let scheduler = Scheduler(clientCheckInterval: 0.05)
        let model = AppModel(environment: [:], home: tmpRoot.path, appVersion: "9.9.9", defaults: MemoryPrefs(),
                             hooks: makeHooks(HookLog(), ScriptedPrompter([])), transport: tiny, scheduler: scheduler)
        return (tiny, model, scheduler)
    }
    let (later, laterModel, _) = clientModel(next: now + 3600)
    await laterModel.start()
    try? await Task.sleep(nanoseconds: 300_000_000)
    check("还没到时间：一次也不起进程", laterModel.clientUpdates?.nextCheckEpoch == now + 3600 && !later.requests.contains { $0.arguments.contains("check") })
    laterModel.shutdown()
    let (past, pastModel, _) = clientModel(next: now - 10)
    await pastModel.start()
    _ = await waitUntil { past.requests.contains { $0.arguments == ["clients", "check", "--json"] } }
    check("到时间了就检查（低优先级）", past.requests.first { $0.arguments == ["clients", "check", "--json"] }?.background == true)
    pastModel.shutdown()

    section("定时拉维护者的映射")
    check("下次的时间是 cca 给的", PolicyPolling.delay(next: now + 3000, now: now) == 3000)
    check("到期了、没拉过：过一会儿再来，不抢着在启动时干活", PolicyPolling.delay(next: now - 5, now: now) == PolicyPolling.minDelay && PolicyPolling.delay(next: nil, now: now) == PolicyPolling.minDelay)
    check("没到时间不拉，到了才拉", !PolicyPolling.isDue(next: now + 1, now: now) && PolicyPolling.isDue(next: now - 1, now: now) && PolicyPolling.isDue(next: nil, now: now))
    let autoPull = CcaCommand.policyPull(manual: false), manualPull = CcaCommand.policyPull(manual: true)
    check("命令：cca policy pull --json，定时的低优先级，手动的不降，都和同步路由器排队",
          autoPull.arguments == ["policy", "pull", "--json"] && manualPull.arguments == autoPull.arguments && autoPull.isBackground && !manualPull.isBackground
          && autoPull.gate == CcaCommand.routerSync.gate && manualPull.gate == CcaCommand.routerSync.gate)
    let pullDoc = #"{"version":1,"kind":"policy_pull","generated_epoch":1794042000,"changed":true,"summary":"Claude 1 个账号"}"#
    let helloForms = tinyHello.replacingOccurrences(of: #""api":1"#, with: #""api":1,"prompt":1,"forms":1"#)
    func policyModel(configured: Bool, next: Double?, doc: String = pullDoc, temp: Bool = false, hello: String? = nil,
                     pull: ResponseOverride? = nil) -> (ScriptedTransport, AppModel, HookLog) {
        var root = (try? JSONSerialization.jsonObject(with: fixture("state_nocodex.json") ?? Data())) as? [String: Any] ?? [:]
        var config = root["config"] as? [String: Any] ?? [:]
        config["policy"] = ["configured": configured, "repo": "example/cca-policy", "next_check_epoch": next as Any? ?? NSNull()] as [String: Any]
        root["config"] = config
        let st = String(decoding: (try? JSONSerialization.data(withJSONObject: root)) ?? Data(), as: UTF8.self)
        let t = ScriptedTransport { r, _ in
            if r.arguments == ["_gui", "hello"] { return response(0, stdout: hello ?? helloForms) }
            if r.arguments == ["_gui", "state"] { return response(0, stdout: st) }
            if r.arguments == ["policy", "pull", "--json"] { return pull?() ?? response(0, stdout: doc) }
            return response(0)
        }
        let log = HookLog()
        if temp { log.conditions = SystemConditions(thermalPressure: true) }
        return (t, makeModel(t, log: log), log)
    }
    func pulls(_ t: ScriptedTransport) -> [CcaRequest] { t.requests.filter { $0.arguments == ["policy", "pull", "--json"] } }
    do {
        let (t, m, _) = policyModel(configured: true, next: now - 5, hello: tinyHello)
        await m.connect()
        check("cca 太旧（没有 policy pull --json）：不定时，也没有手动拉取的按钮", !m.scheduler.policyPollingRunning && !m.policyPullAvailable)
        m.pullPolicyNow()
        try? await Task.sleep(nanoseconds: 60_000_000)
        check("cca 太旧：不拉", pulls(t).isEmpty)
        m.shutdown()
    }
    do {
        let (t, m, _) = policyModel(configured: false, next: nil)
        await m.connect()
        check("没配置映射：不定时", !m.scheduler.policyPollingRunning)
        m.scheduler.onPolicyTick?()
        try? await Task.sleep(nanoseconds: 60_000_000)
        check("没配置映射：不拉", pulls(t).isEmpty)
        m.shutdown()
    }
    do {
        let (t, m, _) = policyModel(configured: true, next: now + 3000)
        await m.connect()
        check("配置了映射：定时拉", m.scheduler.policyPollingRunning)
        m.scheduler.onPolicyTick?()
        try? await Task.sleep(nanoseconds: 60_000_000)
        check("没到 cca 给的时间：不拉（到期后紧接着的那一次 tick 也不重复拉）", pulls(t).isEmpty)
        m.shutdown()
        check("退出时停下", !m.scheduler.policyPollingRunning)
    }
    do {
        let (t, m, _) = policyModel(configured: true, next: now - 5)
        await m.connect()
        m.scheduler.onPolicyTick?()
        _ = await waitUntil { !pulls(t).isEmpty && m.toast != nil }
        check("到时间了：低优先级拉一次，带事件文件（不弹框）；有变化时提示一句", pulls(t).count == 1 && pulls(t)[0].background && pulls(t)[0].wantsEvents && m.toast?.text.contains("维护者更新了节点映射") == true)
        m.shutdown()
    }
    do {
        let (t, m, _) = policyModel(configured: true, next: now - 5, temp: true)
        await m.connect()
        m.scheduler.onPolicyTick?()
        try? await Task.sleep(nanoseconds: 60_000_000)
        check("系统过热：不拉", pulls(t).isEmpty)
        m.pullPolicyNow()
        _ = await waitUntil { !pulls(t).isEmpty }
        check("手动拉取：过热也照拉（是用户点的）", pulls(t).count == 1)
        m.shutdown()
    }
    do {
        let (t, m, _) = policyModel(configured: true, next: now + 3000)
        await m.connect()
        check("手动拉取：配置了映射、cca 够新时有按钮", m.policyPullAvailable)
        m.pullPolicyNow()
        m.pullPolicyNow()
        check("手动拉取：拉的时候按钮显示进行中，点两下只拉一次", m.policyPulling)
        _ = await waitUntil { !m.policyPulling && m.toast != nil }
        check("手动拉取：没到定时的时间也拉，不降优先级；有变化时提示摘要", pulls(t).count == 1 && !pulls(t)[0].background && pulls(t)[0].wantsEvents
              && m.toast?.text == "已更新映射（Claude 1 个账号）")
        m.shutdown()
    }
    do {
        let same = #"{"version":1,"kind":"policy_pull","generated_epoch":1794042000,"changed":false,"summary":"Claude 1 个账号"}"#
        let (_, m, _) = policyModel(configured: true, next: now + 3000, doc: same)
        await m.connect()
        m.pullPolicyNow()
        _ = await waitUntil { !m.policyPulling && m.toast != nil }
        check("手动拉取：没有变化时也照实说", m.toast?.text == "映射没有变化（Claude 1 个账号）")
        m.shutdown()
    }
    do {
        let line = #"{"v":1,"ev":"fail","code":"policy_unavailable","message":"没有拉到映射：连不上 GitHub（接着用上次拉到的）"}"#
        let failed: ResponseOverride = { CcaResponse(exitCode: 1, stdout: Data(), stderr: Data(), events: [EventLog.parseLine(Data(line.utf8))!]) }
        let (_, m, _) = policyModel(configured: true, next: now + 3000, pull: failed)
        await m.connect()
        m.pullPolicyNow()
        _ = await waitUntil { !m.policyPulling && m.sheet != nil }
        if case .failure(let info)? = m.sheet {
            check("手动拉取：拉不到时弹框说原因", info.title == "没能拉取映射" && info.failure.message.contains("连不上 GitHub"))
        } else {
            check("手动拉取：拉不到时弹框说原因", false, "\(String(describing: m.sheet))")
        }
        m.shutdown()
    }
    do {
        let ok = #"{"version":1,"kind":"policy_pull","generated_epoch":1794042000,"changed":true,"summary":"Claude 1 个账号"}"#
        let line = #"{"v":1,"ev":"consent","item":"router_reload","scope":"OpenClash","grant":"router_reload:OpenClash","title":"需要重载 OpenClash","question":"现在重载吗？","detail":"","severity":"warn","default":"yes"}"#
        let consent: ResponseOverride = { CcaResponse(exitCode: 0, stdout: Data(ok.utf8), stderr: Data(), events: [EventLog.parseLine(Data(line.utf8))!]) }
        let (t, m, _) = policyModel(configured: true, next: now + 3000, pull: consent)
        await m.connect()
        m.pullPolicyNow()
        _ = await waitUntil { t.requests.contains { $0.arguments == ["router", "sync"] } }
        check("手动拉取：路由器要重载时接着走“同步路由器”（这次可以问用户）", t.requests.contains { $0.arguments == ["router", "sync"] })
        m.shutdown()
    }
    do {
        let line = #"{"v":1,"ev":"consent","item":"router_reload","scope":"OpenClash","grant":"router_reload:OpenClash","title":"需要重载 OpenClash","question":"现在重载吗？","detail":"","severity":"warn","default":"yes"}"#
        let consent: ResponseOverride = { CcaResponse(exitCode: 0, stdout: Data(pullDoc.utf8), stderr: Data(), events: [EventLog.parseLine(Data(line.utf8))!]) }
        let (t, m, _) = policyModel(configured: true, next: now - 5, pull: consent)
        await m.connect()
        m.scheduler.onPolicyTick?()
        _ = await waitUntil { !pulls(t).isEmpty && !m.policyPulling }
        try? await Task.sleep(nanoseconds: 60_000_000)
        check("定时拉取：路由器要重载时不弹框、不接着同步（留着待同步）", !t.requests.contains { $0.arguments == ["router", "sync"] })
        m.shutdown()
    }
}
