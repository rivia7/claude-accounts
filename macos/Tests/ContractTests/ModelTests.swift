import Foundation

// AppModel：用演示数据（FixtureTransport）和脚本化的 transport。

@MainActor func modelTests() async {
    section("AppModel")
    let bobRef = AccountRef(tool: .claude, key: "工作(bob@example.com)")
    let carolRef = AccountRef(tool: .claude, key: "carol@example.com")
    let frankRef = AccountRef(tool: .codex, key: "frank@example.com")
    let log = HookLog()
    let m = makeModel(fixtureTransport(), log: log)
    await m.start()
    check("启动：ready、读到 state", m.phase == .ready && m.state != nil && m.hello != nil)
    check("演示模式不找 cca", m.isFixtureMode && m.ccaPath == nil)
    check("两个工具都显示", m.visibleTools == [.claude, .codex])
    check("版本不同要提示更新", m.versionMismatch)
    check("默认打开办公室（首页）", m.selection == .office, "\(String(describing: m.selection))")
    check("菜单栏标记（待同步、92%）", m.needsAttention)
    check("API 账号没有用量", m.usage(for: m.account(AccountRef(tool: .claude, key: "香港接口"))!) == nil)
    check("用缓存的用量", m.usage(for: m.account(bobRef)!)?.source == .cache)
    m.refreshUsage(tool: .claude, manual: true)
    check("刷新中", m.usageRefreshing.contains(.claude) && m.isUsageRefreshing(bobRef))
    _ = await waitUntil { m.usageRefreshing.isEmpty && !m.stateLoading }
    check("查到的比缓存新就用查到的", m.usage(for: m.account(bobRef)!)?.source == .live)
    check("没登录的账号不显示用量", m.usage(for: m.account(carolRef)!) == nil)
    m.openApp(bobRef)
    check("打开中", m.activity(bobRef)?.kind == .app)
    _ = await waitUntil { m.activity(bobRef) == nil }
    check("打开成功的提示用 cca 给的话", m.toast?.text == "已打开 工作 的 Claude App，出口 US-B（端口 7901）", m.toast?.text ?? "nil")
    check("提示条会播报给读屏软件", log.announced.last == m.toast?.text)
    m.openBilling(bobRef)
    check("打开账单页中，按钮旁边说的是账单页", m.activity(bobRef)?.kind == .billing && Texts.activity(Activity(kind: .billing), appLabel: "Claude App") == "打开账单页…")
    _ = await waitUntil { m.activity(bobRef) == nil }
    check("账单页打开后不留在进行中，提示用 cca 给的话", m.activity(bobRef) == nil && m.toast?.text.isEmpty == false)
    m.openLimitResets(bobRef)
    check("打开用量页中，按钮旁边说的是用量页", m.activity(bobRef)?.kind == .limitResets && Texts.activity(Activity(kind: .limitResets), appLabel: "Claude App") == "打开用量页…")
    _ = await waitUntil { m.activity(bobRef) == nil }
    check("用量页打开后不留在进行中", m.activity(bobRef) == nil)
    let yes = ScriptedPrompter([true])
    m.hooks.prompter = yes
    m.syncRouter()
    _ = await waitUntil { m.syncActivity == nil }
    check("同步路由器：同意后成功", m.toast?.text == "路由器已同步" && yes.asked.first?.grant == "router_reload:OpenClash")
    m.hooks.prompter = ScriptedPrompter([false])
    m.syncRouter()
    _ = await waitUntil { m.syncActivity == nil }
    check("同步路由器：不同意", m.toast?.text == "没有同意，没有做任何改动")
    m.checkExit(bobRef)
    _ = await waitUntil { m.activity(bobRef) == nil }
    check("检测出口：提示里说共用出口", (m.toast?.text ?? "").contains("共用出口 IP：203.0.113.7"), m.toast?.text ?? "nil")
    m.tool = .codex
    check("办公室不随工具变（切工具只换侧栏的名册）", m.selection == .office)
    m.show(bobRef)
    check("点工位：打开账号页，侧栏的工具跟着换", m.selection == .account(bobRef) && m.tool == .claude)
    m.tool = .codex
    check("换到 ChatGPT 选中它的账号", m.selection == .account(frankRef))
    m.selection = .routing
    m.tool = .claude
    check("分流页不随工具变", m.selection == .routing)
    m.runInTerminal(.edit(prefix: []))
    check("演示模式不开终端", log.opened.isEmpty && (m.toast?.text ?? "").contains("演示模式"))
    m.updateAndRelaunch()
    check("演示模式不更新", log.terminated == 0 && !m.updating)
    m.setMainVisible(true)
    check("看得见时定时刷新", m.scheduler.usageRunning && m.scheduler.liveRunning)
    _ = await waitUntil { m.live != nil }
    check("分流页读 live", m.live?.route == .router && m.live?.proxyHost == "192.0.2.10")
    m.liveRoute = .local
    _ = await waitUntil { m.live?.route == .local }
    check("看本机 Clash 那边", m.live?.route == .local)
    m.setMainVisible(false)
    check("看不见时 live 停下，用量还在轮询（放慢）", m.scheduler.usageRunning && !m.scheduler.liveRunning)
    m.copy("cca edit")
    check("复制", log.copied == ["cca edit"])
    m.apply(Remedy.openURL("https://claude.ai/download"))
    check("打开下载页", log.urls.map(\.absoluteString) == ["https://claude.ai/download"])
    m.shutdown()

    // 换别的事件文件：失败、终端交接、低纯度
    let log2 = HookLog()
    let m2 = makeModel(fixtureTransport(["CCA_GUI_FIXTURE_APP": "events_fail.jsonl", "CCA_GUI_FIXTURE_WEB": "events_terminal.jsonl"]),
                       log: log2, appVersion: "2.9.0")
    await m2.start()
    check("版本相同不提示", !m2.versionMismatch)
    m2.openApp(bobRef)
    _ = await waitUntil { m2.activity(bobRef) == nil }
    if case .failure(let info)? = m2.sheet {
        check("失败弹层：port_closed 的补救是同步路由器", info.failure.code == .portClosed && m2.remedy(for: info.failure) == .routerSync
              && log2.shown >= 1)
    } else {
        check("失败弹层：port_closed 的补救是同步路由器", false, "\(String(describing: m2.sheet))")
    }
    m2.openWeb(carolRef)
    _ = await waitUntil { m2.activity(carolRef) == nil }
    if case .handoff(let h)? = m2.sheet {
        check("终端交接弹层", h.arguments == ["login", "carol@example.com"] && h.display == "cca login carol@example.com")
    } else {
        check("终端交接弹层", false, "\(String(describing: m2.sheet))")
    }
    m2.dismissSheet()
    check("关掉弹层", m2.sheet == nil)
    func failure(_ code: FailCode, _ tool: ToolID = .claude, key: String? = nil) -> Failure {
        Failure(reason: .command, code: code, message: "", stderrTail: "", exitCode: 1, tool: tool, key: key)
    }
    if case .handoff(let h) = m2.remedy(for: failure(.loginMismatch, key: "测试(dave@example.com)")) {
        check("登错号的补救：在终端里登录", h.arguments == ["login", "测试(dave@example.com)"] && h.title == "登录 测试")
    } else {
        check("登错号的补救：在终端里登录", false)
    }
    check("node_missing 的补救：编辑 Codex 账号表",
          m2.remedy(for: failure(.nodeMissing, .codex)) == .handoff(.edit(prefix: ["codex"])))
    check("table_invalid 的补救：cca edit", m2.remedy(for: failure(.tableInvalid)) == .handoff(.edit(prefix: [])))
    check("app_missing 的补救：Claude App 给下载页，ChatGPT App 没有", m2.remedy(for: failure(.appMissing)) == .openURL(Remedy.claudeDownload)
          && m2.remedy(for: failure(.appMissing, .codex)) == Remedy.none)
    check("其它失败没有补救", m2.remedy(for: failure(.lowPurity)) == Remedy.none
          && m2.remedy(for: Failure(reason: .loopGuard, code: .portClosed, message: "", stderrTail: "", exitCode: 1, tool: .claude,
                                    key: nil)) == Remedy.none)
    m2.shutdown()

    let no = ScriptedPrompter([false])
    let m3 = makeModel(fixtureTransport(["CCA_GUI_FIXTURE_APP": "events_consent.jsonl"]), prompter: no)
    await m3.start()
    m3.openApp(bobRef)
    _ = await waitUntil { m3.activity(bobRef) == nil }
    check("低纯度：不同意就不启动", m3.toast?.text == "没有同意，没有做任何改动" && no.asked.first?.item == .lowPurity
          && no.asked.first?.defaultYes == false)
    m3.shutdown()

    // 没安装
    let m4 = makeModel(fixtureTransport(["CCA_GUI_FIXTURE_HELLO": "hello_nocodex_notinstalled.json"]))
    await m4.start()
    check("没安装：显示安装引导、不读 state", m4.phase == .notInstalled && m4.state == nil && m4.visibleTools == [.claude])
    m4.shutdown()

    // 脚本化：Codex 没启用、账号表校验不过、看不见时只记脏、后台刷新用量失败
    let stateNoCodex = String(decoding: fixture("state_nocodex.json") ?? Data(), as: UTF8.self)
    let counter = ScriptedTransport { req, _ in
        switch req.arguments {
        case ["_gui", "hello"]: return response(0, stdout: tinyHello)
        case ["_gui", "state"]: return response(0, stdout: stateNoCodex)
        default: return response(2, stderr: "unexpected")
        }
    }
    let m5 = makeModel(counter, appVersion: "9.9.9")
    await m5.start()
    let states = { counter.requests.filter { $0.arguments == ["_gui", "state"] }.count }
    let hellos = { counter.requests.filter { $0.arguments == ["_gui", "hello"] }.count }
    check("Codex 没启用只显示 Claude", m5.phase == .ready && m5.visibleTools == [.claude] && m5.tool == .claude)
    check("账号表校验不过", m5.invalidTables.first?.tool == .claude)
    check("测试注入的 transport 不找 cca", m5.ccaPath == nil)
    let before = states(), hellosBefore = hellos()
    m5.watcherFired(.state)
    m5.watcherFired(.hello)
    try? await Task.sleep(nanoseconds: 200_000_000)
    check("看不见时 ~/.cca 变了只记下", states() == before && hellos() == hellosBefore)
    m5.setMainVisible(true)
    _ = await waitUntil { states() > before }
    check("看得见了再读 state", states() > before)
    check("cca 换过了：看得见时重新跑 hello", hellos() == hellosBefore + 1)
    _ = await waitUntil { m5.usageRefreshing.isEmpty && !m5.stateLoading }
    try? await Task.sleep(nanoseconds: 300_000_000)
    check("看得见时没有缓存的账号自动刷新用量（只查到期的）", counter.requests.contains { $0.arguments.first == "usage" && $0.arguments.last == "--json" && $0.arguments.count > 2 && $0.background })
    check("后台刷新用量失败：记下来（不弹框）", m5.usageFailure[.claude] != nil && m5.sheet == nil)
    let settled = states()
    m5.desktopAppsChanged()
    m5.desktopAppsChanged()
    _ = await waitUntil(3) { states() > settled }
    try? await Task.sleep(nanoseconds: 300_000_000)
    check("App 启动 / 退出的通知合并后读一次 state", states() == settled + 1, "\(states() - settled)")
    check("没有调用 cca codex", !counter.requests.contains { $0.arguments.first == "codex" })
    m5.shutdown()

    // 路由器没配置时 port_closed 不给“同步路由器”
    var noRouter = jsonObject(Data(stateNoCodex.utf8)) as? [String: Any] ?? [:]
    var cfg = noRouter["config"] as? [String: Any] ?? [:]
    var router = cfg["router"] as? [String: Any] ?? [:]
    router["configured"] = false
    cfg["router"] = router
    noRouter["config"] = cfg
    let noRouterState = String(decoding: (try? JSONSerialization.data(withJSONObject: noRouter)) ?? Data(), as: UTF8.self)
    let m6 = makeModel(ScriptedTransport { req, _ in
        req.arguments == ["_gui", "state"] ? response(0, stdout: noRouterState) : response(0, stdout: tinyHello)
    }, appVersion: "9.9.9")
    await m6.start()
    check("没配置路由器：port_closed 没有补救", !m6.routerConfigured && m6.remedy(for: failure(.portClosed)) == Remedy.none)
    m6.shutdown()

    // 接口版本、旧版 cca
    for (label, resp, want) in [("api 2：提示更新", response(0, stdout: tinyHello.replacingOccurrences(of: "\"api\":1", with: "\"api\":2")), AppModel.Phase.unsupported(api: 2)),
                                ("旧版 cca 没有 _gui", response(1, stderr: "未知命令"), AppModel.Phase.unsupported(api: nil))] {
        let mm = makeModel(ScriptedTransport { _, _ in resp })
        await mm.start()
        check(label, mm.phase == want, "\(mm.phase)")
        mm.shutdown()
    }

    // 已经在用了，hello 偶尔超时：保留界面，显示可以重试的提示
    let timeoutOnce = ScriptedTransport { req, i in
        if req.arguments == ["_gui", "hello"] && i > 0 {
            var r = response(143)
            r.timedOut = true
            return r
        }
        return req.arguments == ["_gui", "state"] ? response(0, stdout: stateNoCodex) : response(0, stdout: tinyHello)
    }
    let m9 = makeModel(timeoutOnce, appVersion: "9.9.9")
    await m9.start()
    await m9.connect()
    check("连上之后 hello 超时：保留界面、显示提示", m9.phase == .ready && m9.state != nil && (m9.stateError ?? "").contains("超时"),
          "\(m9.phase) \(String(describing: m9.stateError))")
    m9.shutdown()
    let m10 = makeModel(ScriptedTransport { _, _ in
        var r = response(143)
        r.timedOut = true
        return r
    })
    await m10.start()
    if case .failed(let msg) = m10.phase {
        check("一开始就连不上：显示出错的卡片", msg.contains("超时"))
    } else {
        check("一开始就连不上：显示出错的卡片", false, "\(m10.phase)")
    }
    m10.shutdown()

    // 「更新并重新打开」分两步（PROTOCOL 5）：先 gui update，成功才后台 --relaunch 并退出；失败时 App 照常开着
    for ok in [true, false] {
        let up = ScriptedTransport { req, _ in
            switch req.arguments {
            case ["gui", "update"]:
                return ok ? response(0) : response(1, [.fail(FailEvent(code: .error, message: "下载失败"))], stderr: "curl: (6)\n")
            case ["_gui", "state"]: return response(0, stdout: stateNoCodex)
            default: return response(0, stdout: tinyHello)
            }
        }
        let ulog = HookLog()
        let mu = makeModel(up, log: ulog, appVersion: "1.0.0")
        await mu.start()
        mu.updateAndRelaunch()
        check(ok ? "更新：下载中" : "更新失败：下载中", mu.updating)
        _ = await waitUntil { !mu.updating && (ulog.terminated > 0 || mu.sheet != nil) }
        let step1 = up.requests.first { $0.arguments == ["gui", "update"] }
        check(ok ? "更新：第一步等它跑完（带事件文件、300 秒）" : "更新失败：第一步照常运行", step1?.wantsEvents == true && step1?.timeout == 300)
        if ok {
            let pid = String(ProcessInfo.processInfo.processIdentifier)
            check("更新：成功后才后台 --relaunch 自己的 pid，然后退出",
                  up.detached.map(\.arguments) == [["gui", "update", "--relaunch", pid]] && ulog.terminated == 1)
        } else {
            if case .failure(let info)? = mu.sheet {
                check("更新失败：显示原因、不退出、不 --relaunch", info.failure.message == "下载失败" && ulog.terminated == 0
                      && up.detached.isEmpty)
            } else {
                check("更新失败：显示原因、不退出、不 --relaunch", false, "\(String(describing: mu.sheet))")
            }
        }
        mu.shutdown()
    }

    // 记住选择
    let prefs = MemoryPrefs()
    let m7 = makeModel(fixtureTransport(), prefs: prefs)
    await m7.start()
    m7.selection = .account(carolRef)
    m7.shutdown()
    let m8 = makeModel(fixtureTransport(), prefs: prefs)
    await m8.start()
    check("下次打开先看办公室", m8.selection == .office, "\(String(describing: m8.selection))")
    m8.show(frankRef)
    m8.tool = .claude
    check("在账号页换回 Claude：选中上次看的账号", m8.selection == .account(carolRef), "\(String(describing: m8.selection))")
    m8.shutdown()
}

/// 内置代理（PROTOCOL 7）：state 里本机客户端是 builtin 时起内核，换掉了就停；.core 信号看不见时也读 state；
/// 退出前停掉并等它退出；演示数据不跑。
@MainActor func coreModelTests() async {
    section("AppModel · 内置代理")
    guard let stateData = fixture("state.json"), var st = jsonObject(stateData) as? [String: Any],
          var cfg = st["config"] as? [String: Any] else {
        check("读 state.json", false)
        return
    }
    cfg["local"] = ["configured": true, "client": "builtin", "name": "内置代理"]
    st["config"] = cfg
    let builtinData = (try? JSONSerialization.data(withJSONObject: st)) ?? Data()
    let builtin = String(decoding: builtinData, as: UTF8.self)
    let current = TextBox(builtin)
    let t = ScriptedTransport { req, _ in
        switch req.arguments {
        case ["_gui", "hello"]: return response(0, stdout: tinyHello)
        case ["_gui", "state"]: return response(0, stdout: current.value)
        default: return response(2, stderr: "unexpected")
        }
    }
    let m = makeModel(t, appVersion: "9.9.9")
    func running() -> Bool { if case .running = m.coreStatus { return true }; return false }
    await m.start()
    check("本机客户端是内置代理：起内核（App 看不见时也起）", t.coreStarts == 1 && running() && !m.isVisible)
    await m.loadState()
    check("state 再读一遍、还是内置代理：不重复起", t.coreStarts == 1)
    current.set(String(decoding: stateData, as: UTF8.self))
    await m.loadState()
    check("换成别的客户端：停掉内核（不等）", t.coreHandles.first?.terminated == [false] && m.coreStatus == .off)
    current.set(builtin)
    m.watcherFired(.core)
    check("core/config.yaml 变了（看不见时也算）：重新读 state，又起内核", await waitUntil(3) { t.coreStarts == 2 } && running())
    m.shutdown()
    check("退出前停掉内核并等它退出", t.coreHandles.last?.terminated == [true] && m.coreStatus == .off)

    // 演示数据：本机客户端是内置代理也不跑内核
    let fx = tmpRoot.appendingPathComponent("fixtures-builtin")
    try? FileManager.default.createDirectory(at: fx, withIntermediateDirectories: true)
    try? fixture("hello.json")?.write(to: fx.appendingPathComponent("hello.json"))
    try? builtinData.write(to: fx.appendingPathComponent("state.json"))
    let demo = makeModel(FixtureTransport(directory: fx, stepDelay: 0))
    await demo.start()
    check("演示数据不跑内置代理的内核", demo.phase == .ready && demo.state?.config?.local?.client == .builtin && demo.coreStatus == .off)
    demo.shutdown()
}
