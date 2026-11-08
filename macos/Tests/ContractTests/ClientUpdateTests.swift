import Foundation

func clientUpdateProtocolTests() async {
    section("客户端更新协议与包验签")
    let token = "checked-target-and-sha256"
    check("客户端检查手动强制、后台节流，均不会安装",
          CcaCommand.clientCheck(force: true).arguments == ["clients", "check", "--force", "--json"]
          && CcaCommand.clientCheck(force: false).arguments == ["clients", "check", "--json"]
          && !CcaCommand.clientCheck(force: true).runsToCompletion)
    check("客户端安装保留绑定token、事务跑完，模式切换不安装",
          CcaCommand.clientInstall(token: token).arguments == ["clients", "install", token, "--json"]
          && CcaCommand.clientInstall(token: token).runsToCompletion
          && CcaCommand.clientMode(automatic: false).arguments == ["clients", "mode", "off", "--json"])
    check("所有客户端变更共享闸门", CcaCommand.clientCheck(force: true).gate == CcaCommand.clientInstall(token: token).gate
          && CcaCommand.clientMode(automatic: true).gate == "client-update")
    let current = SampleDocs.clientUpdates.replacingOccurrences(of: "sample-token", with: token)
    let consent = consentEvent("client_update", token, yes: false)
    let transport = ScriptedTransport { request, _ in
        if request.arguments == ["clients", "status", "--json"] { return response(0, stdout: current) }
        if request.grants == ["client_update:" + token] { return response(0, stdout: current) }
        return response(2, [consent])
    }
    let client = CcaClient(transport: transport)
    let status = try? await client.clientUpdates()
    check("未启用Codex账号也能只读查询客户端更新，不调用cca codex", status?.clients.first?.id == "codex"
          && transport.requests.first?.wantsEvents == false && transport.requests.first?.arguments.first == "clients")
    let no = ScriptedPrompter([false])
    let denied = await client.document(.clientInstall(token: token), as: ClientUpdatesDoc.self, prompter: no)
    check("不同意时不重跑安装", denied.1.runs == 1 && no.asked.first?.grant == "client_update:" + token
          && no.asked.first?.defaultYes == false)
    let yes = ScriptedPrompter([true])
    let approved = await client.document(.clientInstall(token: token), as: ClientUpdatesDoc.self, prompter: yes)
    check("安装授权绑定检查token，只带该scope重跑", approved.1.isSuccess && approved.1.runs == 2
          && approved.1.grants == ["client_update:" + token] && transport.requests.last?.runsToCompletion == true)

    let empty = try? DocumentDecoder.decode(ClientUpdatesDoc.self, from: Data(#"{"version":1,"kind":"client_updates","clients":[{"id":"codex","status":"future_status"},false,null]}"#.utf8))
    check("未知状态和缺失字段不会误判可安装，坏数组元素跳过", empty?.clients.count == 1
          && empty?.clients.first?.status == "future_status" && empty?.clients.first?.readyToInstall == false)
    let changed = try? DocumentDecoder.decode(ClientUpdatesDoc.self, from: Data(current.replacingOccurrences(of: "\"staged\"", with: "\"needs_recheck\"").utf8))
    check("重新检查状态不允许复用旧token，文字说明目标已变化", changed?.clients.first?.readyToInstall == false
          && changed?.clients.first?.detail.contains("校验值已变化") == true)

    let archive = tmpRoot.appendingPathComponent("unsigned-client.zip")
    try? Data("not an official package".utf8).write(to: archive)
    let fakeSignature = Data(repeating: 0, count: 64).base64EncodedString()
    check("固定公钥拒绝伪造签名", !ClientUpdateSignature.verify(archive: archive, signature: fakeSignature))
    check("拒绝坏base64、短签名和不存在的包", !ClientUpdateSignature.verify(archive: archive, signature: "!")
          && !ClientUpdateSignature.verify(archive: archive, signature: "AA==")
          && !ClientUpdateSignature.verify(archive: tmpRoot.appendingPathComponent("missing.zip"), signature: fakeSignature))
    check("headless验签参数必须准确且不能覆盖公钥", ClientUpdateSignature.run(arguments: [ClientUpdateSignature.option]) == 2
          && ClientUpdateSignature.run(arguments: [ClientUpdateSignature.option, "relative.zip", fakeSignature]) == 2
          && ClientUpdateSignature.run(arguments: [ClientUpdateSignature.option, archive.path, fakeSignature, "replacement-key"]) == 2
          && ClientUpdateSignature.run(arguments: [ClientUpdateSignature.option, archive.path, fakeSignature]) == 1)
}

@MainActor func clientUpdateModelTests() async {
    section("客户端更新：自动仅检查、手动安装、生命周期")
    let state = String(decoding: fixture("state_nocodex.json") ?? Data(), as: UTF8.self)
    let doc = TextBox(SampleDocs.clientUpdates)
    let t = ScriptedTransport(delay: 0.02) { request, _ in
        switch request.arguments {
        case ["_gui", "hello"]: return response(0, stdout: tinyHello)
        case ["_gui", "state"]: return response(0, stdout: state)
        case ["clients", "status", "--json"], ["clients", "check", "--json"], ["clients", "check", "--force", "--json"]:
            return response(0, stdout: doc.value)
        case ["clients", "mode", "off", "--json"]:
            doc.set(doc.value.replacingOccurrences(of: "\"auto\"", with: "\"off\""))
            return response(0, stdout: doc.value)
        case ["clients", "install", "sample-token", "--json"]:
            doc.set(doc.value.replacingOccurrences(of: "\"staged\"", with: "\"needs_recheck\""))
            return response(1, stdout: doc.value)
        default: return response(2)
        }
    }
    let scheduler = Scheduler(clientCheckInterval: 0.06)
    let model = AppModel(environment: [:], home: tmpRoot.path, appVersion: "9.9.9", defaults: MemoryPrefs(),
                         hooks: makeHooks(HookLog(), ScriptedPrompter([])), transport: t, scheduler: scheduler)
    await model.start()
    check("启动先只读状态；后台检查循环就绪", model.clientUpdates?.mode == "auto" && scheduler.clientChecksRunning
          && t.requests.filter { $0.arguments.first == "clients" }.map(\.arguments) == [["clients", "status", "--json"]])
    _ = await waitUntil { t.requests.contains { $0.arguments == ["clients", "check", "--json"] } && !model.clientUpdatesBusy }
    check("窗口隐藏时检查，不发安装或强制检查命令", !model.isVisible
          && t.requests.contains { $0.arguments == ["clients", "check", "--json"] }
          && !t.requests.contains { $0.arguments.contains("install") || $0.arguments.contains("--force") })
    model.setClientUpdateAutomatic(false)
    _ = await waitUntil { model.clientUpdates?.mode == "off" && !model.clientUpdatesBusy }
    let count = t.requests.count
    try? await Task.sleep(nanoseconds: 180_000_000)
    check("关闭自动检查后不再发后台命令", t.requests.count == count)
    model.checkClientUpdates(manual: true)
    model.checkClientUpdates(manual: true)
    _ = await waitUntil { !model.clientUpdatesBusy }
    check("手动检查强制刷新，重复点击合并为一次", t.requests.filter { $0.arguments == ["clients", "check", "--force", "--json"] }.count == 1)
    model.selection = .clientUpdates
    _ = await waitUntil { !model.clientUpdatesBusy }
    model.tool = .claude
    check("客户端更新页不随账号工具切换", model.selection == .clientUpdates)
    if let entry = model.clientUpdates?.clients.first {
        model.installClientUpdate(entry)
        _ = await waitUntil { !model.clientUpdatesBusy }
        check("目标变化时给出重新检查提示且不自动重试安装", model.clientUpdates?.clients.first?.status == "needs_recheck"
              && model.clientUpdates?.clients.first?.readyToInstall == false && model.toast?.text.contains("重新检查") == true
              && model.toast?.style == .warning)
        model.installClientUpdate(entry)
        check("旧页面token失效后不再发安装命令", t.requests.filter { $0.arguments.contains("install") }.count == 1)
    } else { check("准备好的客户端条目", false) }
    model.shutdown()
    check("退出停止客户端检查定时器", !scheduler.clientChecksRunning)

    let fixtureModel = makeModel(fixtureTransport())
    await fixtureModel.start()
    check("演示模式只读夹具、不启用自动检查", fixtureModel.clientUpdates?.clients.map(\.id) == ["claude", "codex", "claude-cli", "codex-cli"]
          && !fixtureModel.scheduler.clientChecksRunning)
    fixtureModel.installClientUpdate(fixtureModel.clientUpdates!.clients[0])
    check("演示模式不会安装", fixtureModel.clientUpdateActivity == nil && fixtureModel.toast?.text.contains("不会安装") == true)
    let tools = fixtureModel.clientUpdates!.clients.filter(\.isCLI)
    check("终端工具：有新版本时能直接更新，已是最新的不能", tools.map(\.statusText) == ["有新版本", "已是最新版本"]
          && tools.map(\.readyToInstall) == [true, false] && !fixtureModel.clientUpdates!.clients[0].isCLI)
    fixtureModel.shutdown()

    let dev = tinyHello.replacingOccurrences(of: "\"installed\":true", with: "\"installed\":true,\"dev_mode\":true")
    let devTransport = ScriptedTransport { request, _ in
        if request.arguments == ["_gui", "hello"] { return response(0, stdout: dev) }
        if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    let devModel = makeModel(devTransport)
    await devModel.start()
    devModel.checkClientUpdates(manual: false)
    check("开发模式不自动检查", !devModel.scheduler.clientChecksRunning
          && !devTransport.requests.contains { $0.arguments.contains("check") })
    devModel.shutdown()
}
