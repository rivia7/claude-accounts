import Foundation

/// 「共享与同步」：`config.policy` 的 state / accounts / parked，开关调的命令（PROTOCOL 2.2、4.12）
@MainActor func sharingTests() async {
    section("共享与同步")
    func policy(_ json: String) -> PolicyConfig? { try? JSONDecoder().decode(PolicyConfig.self, from: Data(json.utf8)) }
    let old = policy(#"{"configured": true, "repo": "example/cca-policy", "next_check_epoch": null}"#)
    check("旧版 cca（没有 state）：照 configured 当作开着，没有开关", old?.state == .on && old?.isOn == true && old?.switchable == false
          && old?.accounts == 0 && old?.parked == 0)
    let oldOff = policy(#"{"configured": false, "repo": null, "next_check_epoch": null}"#)
    check("旧版 cca、没配置映射：当作关着", oldOff?.state == .off && oldOff?.isOn == false && oldOff?.switchable == false)
    let off = policy(#"{"configured": false, "repo": null, "next_check_epoch": null, "state": "off", "accounts": 0, "parked": 2}"#)
    check("关着：有开关，记着关掉时移出账号表的账号", off?.switchable == true && off?.isOn == false && off?.parked == 2)
    let pending = policy(#"{"configured": true, "repo": "example/cca-policy", "next_check_epoch": null, "state": "pending", "accounts": 0, "parked": 0}"#)
    check("打开了、还没拉到映射：开关是开着的", pending?.isOn == true && pending?.state == .pending && pending?.switchable == true)
    let unknown = policy(#"{"configured": true, "state": "later-thing", "accounts": 3}"#)
    check("不认识的状态：当作开着", unknown?.isOn == true && unknown?.switchable == true && unknown?.accounts == 3)
    check("命令：关掉是 cca policy off（和同步路由器排队、跑完为止、不降优先级），打开是提问会话 cca policy on",
          CcaCommand.policyOff.arguments == ["policy", "off"] && CcaCommand.policyOff.gate == CcaCommand.routerSync.gate
          && CcaCommand.policyOff.runsToCompletion && !CcaCommand.policyOff.isBackground && CcaCommand.policyOff.tool == nil
          && Handoff.policyOn.arguments == ["policy", "on"] && Handoff.policyOn.inApp && Handoff.policyOn.needsForms)
    check("结果、失败、同意的代码", ResultCode.allKnown.contains(.policyOn) && ResultCode.allKnown.contains(.policyOff)
          && FailCode.allKnown.contains(.policyOffBusy) && ConsentItem.allKnown.contains(.policyOff))

    let onState = stateWithPolicy(["configured": true, "repo": "example/cca-policy", "next_check_epoch": NSNull(), "state": "on", "accounts": 2, "parked": 0])
    let offState = stateWithPolicy(["configured": false, "repo": NSNull(), "next_check_epoch": NSNull(), "state": "off", "accounts": 0, "parked": 2])
    let oldState = stateWithPolicy(["configured": true, "repo": "example/cca-policy", "next_check_epoch": NSNull()])
    let offMessage = "已关掉共享：Claude 的 工作（bob@example.com） 移出了账号表（登录留着，打开共享时加回来）"
    let consent = consentEvent("policy_off", "all", yes: false)
    /// state 先是 first，`policy off` 做完以后是 after；policyOff 是 `policy off` 每一次运行的回应
    func sharingModel(first: String, after: String, prompter: Prompter,
                      policyOff: @escaping @Sendable (CcaRequest) -> CcaResponse) -> (ScriptedTransport, AppModel, HookLog) {
        let current = TextBox(first)
        let t = ScriptedTransport { r, _ in
            if r.arguments == ["_gui", "hello"] { return response(0, stdout: tinyHelloForms) }
            if r.arguments == ["_gui", "state"] { return response(0, stdout: current.value) }
            if r.arguments == ["policy", "off"] {
                let resp = policyOff(r)
                if resp.exitCode == 0 { current.set(after) }
                return resp
            }
            return response(0)
        }
        let log = HookLog()
        return (t, makeModel(t, log: log, prompter: prompter), log)
    }
    func offRuns(_ t: ScriptedTransport) -> [CcaRequest] { t.requests.filter { $0.arguments == ["policy", "off"] } }

    do {
        let (t, m, _) = sharingModel(first: oldState, after: oldState, prompter: ScriptedPrompter([true])) { _ in response(0) }
        await m.connect()
        m.setSharing(false)
        try? await Task.sleep(nanoseconds: 60_000_000)
        check("cca 太旧（config.policy 没有 state）：没有这一页，开关什么都不调", !m.sharingAvailable && offRuns(t).isEmpty && t.started.isEmpty)
        m.shutdown()
    }
    do {
        let p = ScriptedPrompter([true])
        let (t, m, _) = sharingModel(first: onState, after: offState, prompter: p) { r in
            r.grants.isEmpty ? response(2, [consent])
                : response(0, [.result(ResultEvent(code: .policyOff, route: nil, node: nil, port: nil, message: offMessage))])
        }
        await m.connect()
        check("新版 cca：有这一页，开关是开着的", m.sharingAvailable && m.state?.config?.policy?.isOn == true && m.state?.config?.policy?.accounts == 2)
        m.setSharing(false)
        check("关掉：开关先停用", m.sharingBusy)
        _ = await waitUntil { !m.sharingBusy }
        check("关掉：先问同意（cca 给的说明），同意后带着授权重跑一次", p.asked == [consent.consent!] && offRuns(t).count == 2
              && offRuns(t)[0].grants.isEmpty && offRuns(t)[1].grants == ["policy_off:all"] && offRuns(t)[1].wantsEvents)
        check("关好了：提示 cca 说的那句话，重新读 state（开关关上、记着移出的账号）", m.toast?.text == offMessage
              && m.state?.config?.policy?.isOn == false && m.state?.config?.policy?.parked == 2)
        m.setSharing(false)
        try? await Task.sleep(nanoseconds: 60_000_000)
        check("已经关着：再拨一次不调命令", offRuns(t).count == 2)
        m.shutdown()
    }
    do {
        let p = ScriptedPrompter([false])
        let (t, m, _) = sharingModel(first: onState, after: offState, prompter: p) { _ in response(2, [consent]) }
        await m.connect()
        m.setSharing(false)
        _ = await waitUntil { !m.sharingBusy && m.toast != nil }
        check("不同意：不重跑，什么都没改", offRuns(t).count == 1 && m.toast?.text == "没有同意，没有做任何改动" && m.state?.config?.policy?.isOn == true)
        m.shutdown()
    }
    do {
        let busy = "这些共享账号正在用：Claude 的 工作（bob@example.com）（Claude App）。先结束会话、退出 App，再关掉共享"
        let (_, m, _) = sharingModel(first: onState, after: offState, prompter: ScriptedPrompter([true])) { r in
            r.grants.isEmpty ? response(2, [consent]) : response(1, [.fail(FailEvent(code: .policyOffBusy, message: busy))], stderr: busy)
        }
        await m.connect()
        m.setSharing(false)
        _ = await waitUntil { m.sheet != nil }
        if case .failure(let info)? = m.sheet {
            check("共享的账号正在用：弹框说 cca 给的原因（是哪些账号），开关还开着", info.title == "没能关掉共享" && info.failure.code == .policyOffBusy
                  && info.failure.message == busy && m.state?.config?.policy?.isOn == true)
        } else {
            check("共享的账号正在用：弹框说 cca 给的原因（是哪些账号），开关还开着", false)
        }
        m.shutdown()
    }
    do {
        let (t, m, _) = sharingModel(first: offState, after: offState, prompter: ScriptedPrompter([])) { _ in response(0) }
        await m.connect()
        m.setSharing(true)
        _ = await waitUntil { !t.started.isEmpty }
        check("打开：提问会话 cca policy on（问仓库、给维护者的那一行都在会话里），不调 policy off", t.started == [["policy", "on"]] && offRuns(t).isEmpty)
        m.shutdown()
    }
    do {
        let m = makeModel(fixtureTransport())
        await m.start()
        check("演示数据：有这一页，开关开着（2 个共享的账号）", m.sharingAvailable && m.state?.config?.policy?.isOn == true && m.state?.config?.policy?.accounts == 2)
        m.setSharing(false)
        check("演示模式：开关不运行 cca，只提示", m.toast?.text == "演示模式：不会关掉共享" && !m.sharingBusy && m.flow == nil)
        m.shutdown()
    }
}
