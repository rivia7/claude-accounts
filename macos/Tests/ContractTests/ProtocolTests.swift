import Foundation

func rerunProtocolTests() async {
    section("重跑流程")
    let reload = consentEvent("router_reload", "OpenClash")

    // 同意 → 带授权重跑 → 成功
    do {
        let t = ScriptedTransport { req, _ in
            req.grants.contains("router_reload:OpenClash")
                ? response(0, [.step(StepEvent(code: .open)), .result(ResultEvent(code: .opened, route: .router, node: "US-A", port: 7901, message: "ok"))])
                : response(3, [.step(StepEvent(code: .sync)), reload, .fail(FailEvent(code: .syncFailed, message: "没有重载"))])
        }
        let c = CcaClient(transport: t)
        let p = ScriptedPrompter([true])
        let sink = EventSink()
        let r = await c.execute(.app(.claude, key: "alice@example.com"), prompter: p, onEvent: { sink.add($0) })
        check("同意后重跑成功", r.status == .success && r.runs == 2)
        check("第一次不带授权、第二次带上", t.requests.count == 2 && t.requests[0].grants.isEmpty && t.requests[1].grants == ["router_reload:OpenClash"])
        check("每次都要事件文件", t.requests.allSatisfy(\.wantsEvents))
        check("只问了一次", p.asked == [reload.consent!])
        check("结果事件", r.resultEvent?.code == .opened && r.resultEvent?.port == 7901)
        check("事件按顺序交出", sink.events.count == 5 && sink.events.first?.step?.code == .sync && sink.events.last?.result != nil)
        check("参数", t.requests[0].arguments == ["app", "alice@example.com"] && t.requests[0].timeout == 120)
    }
    // 不同意：停下、不重跑
    do {
        let t = ScriptedTransport { _, _ in response(3, [reload]) }
        let r = await CcaClient(transport: t).execute(.web(.claude, key: "k"), prompter: ScriptedPrompter([false]))
        check("不同意就停下", r.status == .refused(reload.consent!) && r.runs == 1 && t.requests.count == 1)
    }
    // 同一个授权再次被要求：防止死循环
    do {
        let t = ScriptedTransport { _, _ in response(3, [reload]) }
        let p = ScriptedPrompter([true, true, true])
        let r = await CcaClient(transport: t).execute(.routerSync, prompter: p)
        expectFailure("同一个授权再次被要求就停下", r.status) { $0.reason == .loopGuard && r.runs == 2 && p.asked.count == 1 }
    }
    // 最多重跑 3 轮
    do {
        let t = ScriptedTransport { _, i in response(3, [consentEvent("router_restart", "S\(i)")]) }
        let p = ScriptedPrompter(Array(repeating: true, count: 10))
        let r = await CcaClient(transport: t).execute(.routerSync, prompter: p)
        expectFailure("最多重跑 3 轮", r.status) { $0.reason == .tooManyRounds && r.runs == 4 && p.asked.count == 3 && t.requests.count == 4 }
        check("最后一次带着前三次的授权", t.requests.last?.grants == ["router_restart:S0", "router_restart:S1", "router_restart:S2"])
    }
    // 一次要两个同意：逐个问，都同意才重跑；第二个不同意就停
    do {
        let lp = consentEvent("low_purity", "203.0.113.20", yes: false)
        let t = ScriptedTransport { req, _ in req.grants.count == 2 ? response(0) : response(1, [reload, lp, reload]) }
        let p = ScriptedPrompter([true, true])
        let r = await CcaClient(transport: t).execute(.app(.claude, key: "k"), prompter: p)
        check("两个同意逐个问、去重", r.status == .success && p.asked.count == 2
              && t.requests.last?.grants == ["router_reload:OpenClash", "low_purity:203.0.113.20"])
        let t2 = ScriptedTransport { _, _ in response(1, [reload, lp]) }
        let p2 = ScriptedPrompter([true, false])
        let r2 = await CcaClient(transport: t2).execute(.app(.claude, key: "k"), prompter: p2)
        check("第二个不同意就停", r2.status == .refused(lp.consent!) && t2.requests.count == 1)
    }
    // 要在终端里完成
    do {
        let term = TerminalEvent(code: .login, argv: ["login", "carol@example.com"], display: "cca login carol@example.com", message: "还没登录")
        let t = ScriptedTransport { _, _ in response(1, [.fail(FailEvent(code: .notLoggedIn, message: "没登录")), .terminal(term)]) }
        let r = await CcaClient(transport: t).execute(.app(.claude, key: "carol@example.com"), prompter: ScriptedPrompter([]))
        check("终端交接", r.status == .terminal(term) && r.runs == 1)
        check("终端交接的 Handoff", Handoff(event: term).arguments == ["login", "carol@example.com"] && Handoff(event: term).display == "cca login carol@example.com")
    }
    // 失败：最后一条 fail，stderr 末尾进“详情”
    do {
        let stderr = (1...30).map { "第 \($0) 行" }.joined(separator: "\n") + "\n"
        let t = ScriptedTransport { _, _ in
            response(1, [.fail(FailEvent(code: .error, message: "先失败")), .fail(FailEvent(code: .portClosed, message: "端口不通"))], stderr: stderr)
        }
        let r = await CcaClient(transport: t).execute(.app(.claude, key: "k"), prompter: ScriptedPrompter([]))
        expectFailure("失败取最后一条 fail", r.status) { $0.code == .portClosed && $0.message == "端口不通" && $0.reason == .command && $0.exitCode == 1 }
        expectFailure("详情是 stderr 的最后几行", r.status) { $0.stderrTail.hasSuffix("第 30 行") && !$0.stderrTail.contains("第 1 行\n") }
        expectFailure("失败带着账号", r.status) { $0.tool == .claude && $0.key == "k" }
        let t2 = ScriptedTransport { _, _ in response(2, [], stderr: "boom\n") }
        let r2 = await CcaClient(transport: t2).execute(.web(.claude, key: "k"), prompter: nil)
        expectFailure("没有 fail 事件时说退出码", r2.status) { $0.message.contains("退出码 2") && $0.stderrTail == "boom" && $0.code == nil }
    }
    // 退出码 0：没用上的 consent 忽略
    do {
        let t = ScriptedTransport { _, _ in response(0, [reload]) }
        let p = ScriptedPrompter([true])
        let r = await CcaClient(transport: t).execute(.app(.claude, key: "k"), prompter: p)
        check("成功时忽略 consent", r.status == .success && p.asked.isEmpty && r.runs == 1)
    }
    // 不弹框（后台刷新）：返回 needsConsent
    do {
        let t = ScriptedTransport { _, _ in response(3, [reload]) }
        let r = await CcaClient(transport: t).execute(.usage(.claude, key: nil), prompter: nil)
        check("不弹框时返回 needsConsent", r.status == .needsConsent([reload.consent!]) && r.runs == 1)
    }
    // usage：退出码 1 但 JSON 能解码 → 成功；要同意时照样先问
    do {
        let t = ScriptedTransport { _, _ in response(1, stdout: tinyUsage) }
        let c = CcaClient(transport: t)
        let (doc, r) = await c.document(.usage(.claude, key: nil), as: UsageDoc.self, prompter: nil)
        check("usage 退出码 1 也收 JSON", r.status == .success && doc?.accounts.first?.errorCode == .port)
        check("usage 的参数", t.requests.first?.arguments == ["usage", "--json"] && t.requests.first?.timeout == 150)
        let t2 = ScriptedTransport { req, _ in
            req.grants.isEmpty ? response(1, [reload], stdout: tinyUsage) : response(0, stdout: tinyUsage)
        }
        let p = ScriptedPrompter([true])
        let (doc2, r2) = await CcaClient(transport: t2).document(.usage(.claude, key: "a@example.com"), as: UsageDoc.self, prompter: p)
        check("usage 要同意时先问再重跑", r2.status == .success && r2.runs == 2 && doc2 != nil && p.asked.count == 1)
        check("usage 单个账号的参数", t2.requests.first?.arguments == ["usage", "a@example.com", "--json"])
        let t3 = ScriptedTransport { _, _ in response(1, stdout: "不是 JSON") }
        let (doc3, r3) = await CcaClient(transport: t3).document(.usage(.claude, key: nil), as: UsageDoc.self, prompter: nil)
        expectFailure("usage 输出坏了算失败", r3.status) { _ in doc3 == nil }
    }
    // Codex 没启用时绝不调用 cca codex …
    do {
        let t = ScriptedTransport { _, _ in response(0) }
        let c = CcaClient(transport: t)
        let r = await c.execute(.app(.codex, key: "frank@example.com"), prompter: nil)
        expectFailure("Codex 没启用时不运行", r.status) { $0.reason == .codexDisabled && t.requests.isEmpty }
        await c.setCodexEnabled(true)
        _ = await c.execute(.app(.codex, key: "frank@example.com"), prompter: nil)
        _ = await c.document(.usage(.codex, key: nil), as: UsageDoc.self, prompter: nil)
        _ = await c.document(.check(.codex, key: "frank@example.com"), as: CheckDoc.self, prompter: nil)
        check("Codex 的参数带 codex", t.requests.map(\.arguments) == [["codex", "app", "frank@example.com"], ["codex", "usage", "--json"],
                                                                       ["codex", "check", "frank@example.com", "--json"]])
        check("check 的超时", t.requests.last?.timeout == 180)
        check("app / web / router sync 跑完为止，usage / check 不是", t.requests.map(\.runsToCompletion) == [true, false, false])
    }
    // 其它命令的参数
    check("web 的参数", CcaCommand.web(.claude, key: "k").arguments == ["web", "k"] && CcaCommand.web(.claude, key: "k").runsToCompletion)
    check("账单页的参数", CcaCommand.billing(.claude, key: "k").arguments == ["billing", "k"] && CcaCommand.billing(.codex, key: "k").arguments == ["codex", "billing", "k"]
          && CcaCommand.billing(.claude, key: "k").runsToCompletion && CcaCommand.billing(.claude, key: "k").timeout == CcaCommand.web(.claude, key: "k").timeout)
    check("router sync 的参数", CcaCommand.routerSync.arguments == ["router", "sync"] && CcaCommand.routerSync.timeout == 180
          && CcaCommand.routerSync.runsToCompletion)
    check("check 全部账号的参数", CcaCommand.check(.claude, key: nil).arguments == ["check", "--json"])
    check("gui update（第一步）", CcaCommand.guiUpdate.arguments == ["gui", "update"] && CcaCommand.guiUpdate.timeout == 300
          && !CcaCommand.guiUpdate.runsToCompletion && CcaCommand.guiUpdate.gate == "gui-update")
    let reset = CcaCommand.reset(.codex, key: "frank@example.com")
    check("重置额度：参数、带 codex、跑完为止（同意之后就会用掉一次）、同一个账号一次一个",
          reset.arguments == ["codex", "reset", "frank@example.com", "--json"] && reset.runsToCompletion
          && reset.gate == "reset:codex:frank@example.com" && reset.tool == .codex && reset.key == "frank@example.com")
    check("更新 cca 到某个版本：参数、超时，和 gui update 一个闸门（一个接着一个）",
          CcaCommand.update("2.9.1").arguments == ["update", "2.9.1"] && CcaCommand.update("2.9.1").timeout == 300
          && !CcaCommand.update("2.9.1").runsToCompletion && CcaCommand.update("2.9.1").gate == "gui-update")
    check("检查更新：参数、超时，不排队", CcaCommand.updateCheck.arguments == ["update", "--check", "--json"]
          && CcaCommand.updateCheck.timeout == 150 && !CcaCommand.updateCheck.runsToCompletion && CcaCommand.updateCheck.gate == nil)
    check("前缀只有一处", ToolID.claude.defaultPrefix.isEmpty && ToolID.codex.defaultPrefix == ["codex"])
    // 只读查询
    do {
        let t = ScriptedTransport { req, _ in
            switch req.arguments {
            case ["_gui", "hello"]: return response(0, stdout: tinyHello)
            case ["_gui", "state"]: return response(1, stderr: "内部错误\n")
            default: return response(0, stdout: tinyHello)
            }
        }
        let c = CcaClient(transport: t)
        let h = try? await c.hello()
        check("hello", h?.ccaVersion == "9.9.9" && t.requests[0].timeout == 5 && !t.requests[0].wantsEvents && t.requests[0].grants.isEmpty)
        do {
            _ = try await c.state()
            check("state 退出码非 0 报错", false)
        } catch let e as CcaError {
            check("state 退出码非 0 报错", e == .exit(1, stderrTail: "内部错误"), "\(e)")
        } catch {
            check("state 退出码非 0 报错", false, "\(error)")
        }
        do {
            _ = try await c.live(route: .local)
            check("live 收到 hello 时报错", false)
        } catch let e as CcaError {
            if case .badOutput = e { check("live 收到 hello 时报错", true) } else { check("live 收到 hello 时报错", false, "\(e)") }
        } catch {
            check("live 收到 hello 时报错", false)
        }
        check("live 指定看哪边用 CCA_ROUTE", t.requests.last?.extraEnvironment == ["CCA_ROUTE": "local"] && t.requests.last?.timeout == 30)
        try? await c.relaunchAfterUpdate(pid: 123)
        check("gui update 后台启动", t.detached.first?.arguments == ["gui", "update", "--relaunch", "123"])
    }
}

func gateTests() async {
    section("闸门")
    func overlaps(_ a: (label: String, start: Double, end: Double), _ b: (label: String, start: Double, end: Double)) -> Bool {
        max(a.start, b.start) < min(a.end, b.end) - 0.05
    }
    func pair(_ x: CcaCommand, _ y: CcaCommand) async -> Bool {
        let t = ScriptedTransport(delay: 0.3) { _, _ in response(0) }
        let c = CcaClient(transport: t)
        await c.setCodexEnabled(true)
        async let a = c.execute(x, prompter: nil)
        async let b = c.execute(y, prompter: nil)
        _ = await (a, b)
        let log = t.log
        return log.count == 2 && overlaps(log[0], log[1])
    }
    check("同一个账号的 app 排队", !(await pair(.app(.claude, key: "k1"), .app(.claude, key: "k1"))))
    check("同一个账号的 app 和 web 排队", !(await pair(.app(.claude, key: "k1"), .web(.claude, key: "k1"))))
    check("同一个账号的网页版和账单页排队（是同一份浏览器资料）", !(await pair(.web(.claude, key: "k1"), .billing(.claude, key: "k1"))))
    check("不同账号的 app 同时跑", await pair(.app(.claude, key: "k1"), .app(.claude, key: "k2")))
    check("Claude 和 Codex 的同一个键是不同账号", await pair(.app(.claude, key: "k1"), .app(.codex, key: "k1")))
    check("同一个工具的 usage 排队", !(await pair(.usage(.claude, key: nil), .usage(.claude, key: "k1"))))
    check("Claude 和 Codex 的 usage 同时跑", await pair(.usage(.claude, key: nil), .usage(.codex, key: nil)))
    check("router sync 全局一个", !(await pair(.routerSync, .routerSync)))
    check("check 不排队", await pair(.check(.claude, key: "k1"), .check(.claude, key: "k1")))
    check("同一个账号的重置额度排队", !(await pair(.reset(.codex, key: "k1"), .reset(.codex, key: "k1"))))
    check("不同账号的重置额度同时跑", await pair(.reset(.codex, key: "k1"), .reset(.codex, key: "k2")))
    check("更新 cca 和 gui update 排队（一个接着一个）", !(await pair(.update("9.9.10"), .guiUpdate)))
}
