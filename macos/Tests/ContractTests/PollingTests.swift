import Foundation

// 自动刷新用量、检查客户端更新的节奏：只查到期的账号，系统吃紧、用户走开时不查，后台的活低优先级，不空转。

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
}
