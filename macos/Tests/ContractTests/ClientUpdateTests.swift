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
          && CcaCommand.clientMode(automatic: true).gate == "client-update"
          && CcaCommand.clientAutoInstall(enabled: true).gate == "client-update")
    check("允许自动更新：开关只改设置；检查参数不变，但可能顺带安装，要跑完为止",
          CcaCommand.clientAutoInstall(enabled: true).arguments == ["clients", "auto-install", "on", "--json"]
          && CcaCommand.clientAutoInstall(enabled: false).arguments == ["clients", "auto-install", "off", "--json"]
          && !CcaCommand.clientAutoInstall(enabled: true).runsToCompletion && CcaCommand.clientAutoInstall(enabled: true).timeout == 30
          && CcaCommand.clientCheck(force: false, install: true).arguments == ["clients", "check", "--json"]
          && CcaCommand.clientCheck(force: false, install: true).runsToCompletion
          && CcaCommand.clientCheck(force: false, install: true).isBackground
          && !CcaCommand.clientCheck(force: true, install: true).isBackground
          && !CcaCommand.clientCheck(force: false).runsToCompletion)
    let waiting = try? DocumentDecoder.decode(ClientUpdatesDoc.self, from: Data(SampleDocs.clientUpdates
        .replacingOccurrences(of: #""mode":"auto","#, with: #""mode":"auto","auto_install":true,"#)
        .replacingOccurrences(of: #""can_install":true,"#, with: #""can_install":true,"auto_install_pending":true,"#).utf8))
    let plain = try? DocumentDecoder.decode(ClientUpdatesDoc.self, from: Data(SampleDocs.clientUpdates.utf8))
    check("解码允许自动更新和等客户端退出：旧版 cca 没有这两项时没有开关、不在等",
          waiting?.autoInstall == true && waiting?.autoInstallPending == true && waiting?.clients.first?.statusText == "退出后自动安装"
          && waiting?.clients.first?.readyToInstall == true
          && plain?.autoInstall == nil && plain?.autoInstallPending == false && plain?.clients.first?.statusText == "可以安装了")
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
          && changed?.clients.first?.detail.contains("要安装的版本变了") == true)

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

@MainActor func clientAutoInstallModelTests() async {
    section("客户端更新：允许自动更新")
    let state = String(decoding: fixture("state_nocodex.json") ?? Data(), as: UTF8.self)
    let off = SampleDocs.clientUpdates.replacingOccurrences(of: #""mode":"auto","#, with: #""mode":"auto","auto_install":false,"#)
    let on = off.replacingOccurrences(of: #""auto_install":false"#, with: #""auto_install":true"#)
    let waiting = on.replacingOccurrences(of: #""can_install":true,"#, with: #""can_install":true,"auto_install_pending":true,"#)
    let done = on.replacingOccurrences(of: #""status":"staged""#, with: #""status":"updated""#)
        .replacingOccurrences(of: #""can_install":true"#, with: #""can_install":false"#)
    let doc = TextBox(off)
    let open = TextBox("1")   // ChatGPT 还开着
    let t = ScriptedTransport(delay: 0.02) { request, _ in
        switch request.arguments {
        case ["_gui", "hello"]: return response(0, stdout: tinyHello)
        case ["_gui", "state"]: return response(0, stdout: state)
        case ["clients", "status", "--json"]: return response(0, stdout: doc.value)
        case ["clients", "auto-install", "on", "--json"]: doc.set(on); return response(0, stdout: doc.value)
        case ["clients", "auto-install", "off", "--json"]: doc.set(off); return response(0, stdout: doc.value)
        case ["clients", "check", "--json"], ["clients", "check", "--force", "--json"]:
            if doc.value != off { doc.set(open.value == "1" ? waiting : done) }
            return response(0, stdout: doc.value)
        default: return response(2)
        }
    }
    let scheduler = Scheduler(clientCheckInterval: 3600)
    let model = AppModel(environment: [:], home: tmpRoot.path, appVersion: "9.9.9", defaults: MemoryPrefs(),
                         hooks: makeHooks(HookLog(), ScriptedPrompter([])), transport: t, scheduler: scheduler)
    model.autoInstallSettle = 0.01
    await model.start()
    check("旧设置：不允许自动更新，检查不必跑完为止", model.clientUpdates?.autoInstall == false)
    model.desktopAppsChanged(terminated: true)
    try? await Task.sleep(nanoseconds: 150_000_000)
    check("没在等客户端退出：客户端退出不触发检查", !t.requests.contains { $0.arguments.first == "clients" && $0.arguments.contains("check") })

    model.setClientUpdateAutoInstall(true)
    _ = await waitUntil { model.clientUpdates?.autoInstallPending == true && !model.clientUpdatesBusy }
    let checks = { t.requests.filter { $0.arguments == ["clients", "check", "--json"] } }
    check("打开后马上在后台检查一次（不强制），这次检查要跑完为止", t.requests.contains { $0.arguments == ["clients", "auto-install", "on", "--json"] }
          && checks().count == 1 && checks().first?.runsToCompletion == true
          && model.toast?.text.contains("已允许自动更新") == true)
    check("客户端开着：页面说退出后自动安装", model.clientUpdates?.clients.first?.statusText == "退出后自动安装")
    model.desktopAppsChanged(terminated: false)
    try? await Task.sleep(nanoseconds: 150_000_000)
    check("客户端打开不触发检查", checks().count == 1)
    open.set("0")
    let states = t.requests.filter { $0.arguments == ["_gui", "state"] }.count
    model.desktopAppsChanged(terminated: true)
    _ = await waitUntil { model.clientUpdates?.clients.first?.status == "updated" && !model.clientUpdatesBusy }
    _ = await waitUntil { t.requests.filter { $0.arguments == ["_gui", "state"] }.count > states }
    check("客户端退出后检查一次，装上了；重读 state", checks().count == 2 && model.clientUpdates?.autoInstallPending == false
          && t.requests.filter { $0.arguments == ["_gui", "state"] }.count > states)

    model.setClientUpdateAutoInstall(false)
    _ = await waitUntil { model.clientUpdates?.autoInstall == false && !model.clientUpdatesBusy }
    try? await Task.sleep(nanoseconds: 100_000_000)
    check("关掉：不再顺带检查，安装前先问", t.requests.contains { $0.arguments == ["clients", "auto-install", "off", "--json"] }
          && checks().count == 2 && model.toast?.text.contains("已关闭自动更新") == true)
    model.checkClientUpdates(manual: true)
    _ = await waitUntil { !model.clientUpdatesBusy }
    check("不允许自动更新时手动检查照旧（不必跑完为止）",
          t.requests.last { $0.arguments == ["clients", "check", "--force", "--json"] }?.runsToCompletion == false)
    model.shutdown()

    let fixtureModel = makeModel(fixtureTransport())
    await fixtureModel.start()
    fixtureModel.setClientUpdateAutoInstall(true)
    check("演示模式不改设置", fixtureModel.clientUpdateActivity == nil && fixtureModel.toast?.text.contains("不会更改自动更新设置") == true)
    fixtureModel.shutdown()
}
