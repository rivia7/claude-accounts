import Foundation

/// 自己的同步服务器（PROTOCOL 2.2 的 `config.hubs`、账号的 `hub`，3.4、3.5 的 `hubs`，4.13 的 `cca hub`）：只在 --full（手写的 Tests/Fixtures）里跑
func hubFixtureTests() {
    section("自己的同步服务器")
    if let h = loadFixture(HelloDoc.self, "hello.json") {
        check("hello：hub（cca 有自己的同步服务器）", h.hub == 1 && h.hub == HubAPI.version)
    }
    if let h = loadFixture(HelloDoc.self, "hello_nocodex_notinstalled.json") {
        check("hello：旧版 cca 没有 hub", h.hub == nil)
    }
    if let s = loadFixture(StateDoc.self, "state.json") {
        let hubs = s.config?.hubs ?? []
        check("state：config.hubs 维护者的在前、自己的在后，不认识的 kind 去掉了", hubs.map(\.kind) == [.maintainer, .own])
        let own = s.config?.ownHub
        check("state：自己的同步服务器没同步成，有原因、上次成功的时间、设备数、加入时间，能给加入链接",
              own?.state == .error && own?.state.failing == true && own?.error?.contains("连不上") == true && own?.checkedEpoch == 1794030000
              && own?.members == 3 && own?.joinedEpoch == 1793900000 && own?.canInvite == true && own?.host == "hub.example.com:18443")
        check("state：维护者的服务器没有设备数、加入时间、加入链接", hubs.first?.members == nil && hubs.first?.joinedEpoch == nil
              && hubs.first?.canInvite == false && hubs.first?.name == "studio-mac" && hubs.first?.accounts == 1)
        let bob = s.account(AccountRef(tool: .claude, key: "工作(bob@example.com)"))
        let carol = s.account(AccountRef(tool: .claude, key: "carol@example.com"))
        let erin = s.account(AccountRef(tool: .claude, key: "erin@example.com"))
        check("state：bob 经维护者的服务器同步，折合金额也是", bob?.hub == .maintainer && bob?.cost?.sync?.hub == .maintainer)
        check("state：carol 不经服务器同步", carol?.hub == nil)
        check("state：只在这台设备上的折合金额（local_only）不说经哪台", erin?.cost?.sync?.state == .localOnly && erin?.cost?.sync?.hub == nil)
    }
    if let s = loadFixture(StateDoc.self, "state_cost_sync.json") {
        let alice = s.account(AccountRef(tool: .claude, key: "alice@example.com"))
        check("旧版 cca 的 state（没有 hub、config.hubs）：有「设备」一节的账号当作经维护者的服务器，折合金额也是；config.hubs 是空的",
              alice?.hub == .maintainer && alice?.can.devices == true && alice?.cost?.sync?.hub == .maintainer && s.config?.hubs.isEmpty == true)
    }
    // 以后的 cca 给了不认识的 hub：当作 null（不经服务器同步），有「设备」一节也不当作维护者的
    if let d = fixture("state.json"), let text = String(data: d, encoding: .utf8) {
        let later = Data(text.replacingOccurrences(of: #""hub": "maintainer""#, with: #""hub": "later-kind""#).utf8)
        let s = try? DocumentDecoder.decode(StateDoc.self, from: later)
        let bob = s?.account(AccountRef(tool: .claude, key: "工作(bob@example.com)"))
        check("state：不认识的 hub 当作 null（有「设备」一节也一样），折合金额也是", bob != nil && bob?.can.devices == true && bob?.hub == nil
              && bob?.cost?.sync?.hub == nil)
    }
    if let doc = loadFixture(HubStatusDoc.self, "hub_status.json") {
        check("hub status：加入了、用得了、还没同步过", doc.joined && doc.reason == nil && doc.hub?.kind == .own && doc.hub?.state == .pending
              && doc.hub?.accounts == 2 && doc.hub?.members == 1 && doc.hub?.checkedEpoch == nil)
    }
    let notJoined = try? DocumentDecoder.decode(HubStatusDoc.self, from: Data(#"{"version":1,"kind":"hub_status","joined":false,"reason":"还没加入","hub":null}"#.utf8))
    check("hub status：没加入", notJoined?.joined == false && notJoined?.hub == nil && notJoined?.reason == "还没加入")
    if let doc = loadFixture(HubDevicesDoc.self, "hub_devices.json") {
        check("hub devices：这台在前；服务器重启以后没来过的没有 seen", doc.devices.map(\.name) == ["studio-mac", "carol-air"] && doc.devices[0].me
              && !doc.devices[1].me && doc.devices[1].seenEpoch == nil && doc.devices[0].id.hasPrefix("SHA256:") && doc.devices[0].joinedEpoch == 1793900000)
    }
    if let doc = loadFixture(DevicesDoc.self, "devices.json"), let a = doc.accounts.first {
        let h = doc.hub(for: a)
        check("旧版 cca 的 devices --json（没有 hubs、账号的 hub）：当作维护者的服务器，设置和名字用顶层的",
              doc.hubs.isEmpty && a.hub == nil && h.kind == .maintainer && h.me == doc.me && h.idleAfter == doc.idleAfter && h.offlineAfter == doc.offlineAfter)
    }
    let two = try? DocumentDecoder.decode(DevicesDoc.self, from: Data(#"""
        {"version":1,"kind":"devices","generated_epoch":1,"tool":"claude","me":"m-a","idle_after":600,"offline_after":150,"reporting_error":null,
         "hubs":[{"kind":"maintainer","me":"m-a","idle_after":600,"offline_after":150,"reporting_error":null},
                 {"kind":"own","me":"o-a","idle_after":300,"offline_after":90,"reporting_error":"连不上"}],
         "accounts":[{"key":"a(a@example.com)","hub":"maintainer","devices":[]},{"key":"b(b@example.com)","hub":"own","devices":[]}]}
        """#.utf8))
    if let two, two.accounts.count == 2 {
        let o = two.hub(for: two.accounts[1])
        check("devices --json 两台服务器：各账号按自己的 hub 用那一台的名字和设置", two.hub(for: two.accounts[0]).me == "m-a" && o.kind == .own && o.me == "o-a"
              && o.idleAfter == 300 && o.offlineAfter == 90 && o.reportingError == "连不上")
    } else {
        check("devices --json 两台服务器：解码了", false)
    }
    if let doc = loadFixture(CostSyncDoc.self, "cost_sync.json") {
        check("旧版 cca 的 cost sync --json（没有 hubs）：hubs 是空的", doc.sync?.hubs.isEmpty == true && doc.sync?.kind == nil)
    }
    let sync = try? DocumentDecoder.decode(CostSyncDoc.self, from: Data(#"""
        {"version":1,"kind":"cost_sync","generated_epoch":1,"scan":{"state":"ready","updated_epoch":1,"files":0,"read_bytes":0,"records":0,"first":false},
         "sync":{"state":"error","message":"连不上","checked_epoch":null,"hubs":[{"kind":"maintainer","state":"ok","message":null,"checked_epoch":5},
                                                                          {"kind":"own","state":"error","message":"连不上","checked_epoch":null}]}}
        """#.utf8))
    check("cost sync --json 两台服务器：顶层是最要紧的那一台，hubs 各是各的", sync?.sync?.state == .error && sync?.sync?.hubs.map(\.kind) == [.maintainer, .own]
          && sync?.sync?.hubs.first?.checkedEpoch == 5 && sync?.sync?.hubs.last?.message == "连不上")
    check("结果、失败、同意的代码", [ResultCode.hubJoined, .hubRemoved, .hubLeft, .hubDeployed].allSatisfy(\.isKnown)
          && [FailCode.hubSSH, .hubTaken, .hubNotThis, .hubMoved, .hubStart].allSatisfy { $0.isKnown && $0.isHub } && ConsentItem.hubReset.isKnown
          && [FailCode.hubNotJoined, .hubNoDevice, .hubUnavailable, .hubLink, .hubDenied, .hubSalt].allSatisfy(\.isKnown)
          && FailCode(rawValue: "hub_later").isHub && !FailCode(rawValue: "hub_later").isKnown && FailCode.hubPin.isKnown && !FailCode.devicesOff.isHub && [ConsentItem.hubRemove, .hubLeave].allSatisfy(\.isKnown))
    let homes = ["/Users/me/.cca"]
    let cases: [(String, CcaWatcher.Trigger?)] = [
        ("/Users/me/.cca/cost/own.remote.json", .state), ("/Users/me/.cca/cost/own.sync.json", .state), ("/Users/me/.cca/cost/own.lock", nil),
        ("/Users/me/.cca/hub/hub.json", .state), ("/Users/me/.cca/hub/members.json", .state), ("/Users/me/.cca/hub/id_ed25519", nil),
        ("/Users/me/.cca/hub/hub.json.tmp", nil), ("/Users/me/.cca/presence/status.json", .state), ("/Users/me/.cca/presence/own.status.json", .state),
        ("/Users/me/.cca/presence/keys.json", nil), ("/Users/me/.cca/presence/agent.json", nil), ("/Users/me/.cca/accounts/a/hub/hub.json", nil),
    ]
    let wrong = cases.filter { CcaWatcher.classify(path: $0.0, homes: homes) != $0.1 }.map(\.0)
    check("看着自己的同步服务器的加入信息、设备数、设备统计的状态、经它同步来的折合金额（别的不算）", wrong.isEmpty, wrong.joined(separator: " "))
}

/// 「共享与同步」页上的自己的同步服务器（PROTOCOL 4.13）：调的命令、页面上的几句话
func hubProtocolTests() {
    section("自己的同步服务器：命令和文字")
    let auto = CcaCommand.hubDevices(auto: true), manual = CcaCommand.hubDevices(auto: false)
    check("设备：cca hub devices --json；页面自己查的低优先级，点「刷新」的不降；只读，不占闸门",
          auto.arguments == ["hub", "devices", "--json"] && auto.isBackground && !manual.isBackground && !auto.runsToCompletion
          && auto.gate == nil && auto.tool == nil && auto.key == nil && auto.timeout == 40)
    let fp = "SHA256:" + String(repeating: "B", count: 43)
    let rm = CcaCommand.hubRemove(device: fp)
    check("移除、退出：按设备指纹；都改这台设备的加入信息，一个接一个；不降优先级、不写文件",
          rm.arguments == ["hub", "remove", fp] && CcaCommand.hubLeave.arguments == ["hub", "leave"] && rm.gate == CcaCommand.hubLeave.gate
          && rm.gate != nil && !rm.isBackground && !rm.runsToCompletion && !CcaCommand.hubLeave.runsToCompletion && rm.tool == nil)
    let inv = CcaCommand.hubInvite
    check("加入链接：cca hub invite，输出只经内存里的管道（不写文件）", inv.arguments == ["hub", "invite"] && !inv.runsToCompletion
          && !inv.isBackground && inv.gate == nil && inv.tool == nil)
    let d = Handoff.hubDeploy(reset: false), r = Handoff.hubDeploy(reset: true)
    check("部署、加入是提问会话（App 里问完）；部署要表单，从头部署带 --reset",
          d.arguments == ["hub", "deploy"] && r.arguments == ["hub", "deploy", "--reset"] && d.inApp && r.inApp && d.needsForms && r.needsForms
          && Handoff.hubJoin.arguments == ["hub", "join"] && Handoff.hubJoin.inApp && d.title != r.title)
    check("部署、加入做完了带着的提醒都认识", [FailCode.hubNewpin, .hubLan, .hubOff].allSatisfy { $0.isKnown && $0.isHub })

    guard let own = loadFixture(StateDoc.self, "state.json")?.config?.ownHub else {
        check("演示数据里有自己的同步服务器", false)
        return
    }
    let now = 1794042000.0
    check("没同步成：cca 给的原因，和多久前同步成过", Texts.hubState(own, now: now) == "最近一次没同步成：连不上 https://hub.example.com:18443（超时）；3 小时前同步成过",
          Texts.hubState(own, now: now))
    var ok = own
    ok.state = .ok
    ok.error = nil
    var pending = own
    pending.state = .pending
    pending.checkedEpoch = nil
    var off = own
    off.state = .off
    off.error = "config.json 里 presence 是 false"
    check("正常、还没同步过、这台设备关掉了设备统计", Texts.hubState(ok, now: now) == "同步正常（3 小时前）"
          && Texts.hubState(pending, now: now).hasPrefix("还没同步过") && Texts.hubState(off, now: now) == "config.json 里 presence 是 false：现在不经它同步")
    let cal = Calendar(identifier: .gregorian)
    check("经它同步几个账号、这台在服务器上叫什么、什么时候加入的", Texts.hubSummary(own, now: now, calendar: cal).hasPrefix("还没有经它同步的账号 · 这台 Mac 在服务器上叫 studio-mac · ")
          && Texts.hubSummary(own, now: now, calendar: cal).hasSuffix(" 加入"), Texts.hubSummary(own, now: now, calendar: cal))
    if let doc = loadFixture(HubDevicesDoc.self, "hub_devices.json"), doc.devices.count == 2 {
        check("设备：最近一次同步；服务器重启以后没来过的照实说", Texts.hubDeviceDetail(doc.devices[0], now: now, calendar: cal).hasPrefix("最近一次同步：刚刚 · ")
              && Texts.hubDeviceDetail(doc.devices[1], now: now, calendar: cal).hasPrefix("服务器重启以后还没来过 · "))
    }
}

/// 「共享与同步」页的自己的同步服务器：什么时候问设备、问不到怎么显示、复制加入链接、移除、退出、部署、加入
@MainActor func hubModelTests() async {
    section("自己的同步服务器：「共享与同步」页")
    guard let hello = fixture("hello.json").map({ String(decoding: $0, as: UTF8.self) }),
          let state = fixture("state.json").map({ String(decoding: $0, as: UTF8.self) }),
          let devices = fixture("hub_devices.json").map({ String(decoding: $0, as: UTF8.self) }) else {
        check("夹具", false)
        return
    }
    let link = "cca-hub1." + String(repeating: "Zq", count: 24)   // 运行时拼出来：仓库里不放长得像凭证的字符串
    let removed = "已移除「carol-air」"
    let mode = TextBox("ok")
    func transport(_ hello: String) -> ScriptedTransport {
        ScriptedTransport { r, _ in
            let a = r.arguments
            if a == ["_gui", "hello"] { return response(0, stdout: hello) }
            if a == ["_gui", "state"] { return response(0, stdout: state) }
            if a == ["hub", "devices", "--json"] {
                if mode.value == "fail" {
                    return response(1, [.fail(FailEvent(code: .hubUnavailable, message: "连不上 https://hub.example.com:18443（超时）"))], stderr: "连不上\n")
                }
                return response(0, stdout: devices)
            }
            if a == ["hub", "invite"] { return response(0, stdout: link + "\n") }
            if a.prefix(2) == ["hub", "remove"], a.count == 3 {
                return r.grants.isEmpty ? response(3, [consentEvent("hub_remove", a[2], yes: false)])
                    : response(0, [.result(ResultEvent(code: .hubRemoved, route: nil, node: nil, port: nil, message: removed))])
            }
            if a == ["hub", "leave"] { return response(3, [consentEvent("hub_leave", "all", yes: false)]) }
            return response(0, stdout: SampleDocs.clientUpdates)
        }
    }
    func hubRuns(_ t: ScriptedTransport) -> [[String]] { t.requests.map(\.arguments).filter { $0.first == "hub" } }

    // 旧版 cca（hello 没有 hub）：这一块没有，也不调 cca hub
    do {
        let t = transport(hello.replacingOccurrences(of: #""hub": 1,"#, with: ""))
        let m = makeModel(t)
        await m.start()
        m.setMainVisible(true)
        m.selection = .sharing
        m.refreshHubDevices(manual: true)
        m.copyHubInvite()
        m.joinHub()
        m.deployHub()
        try? await Task.sleep(nanoseconds: 100_000_000)
        check("旧版 cca（hello 没有 hub）：没有自己的同步服务器（state 里有也不认），不调 cca hub、不开会话", !m.hubAvailable && m.ownHub == nil
              && hubRuns(t).isEmpty && t.started.isEmpty)
        m.shutdown()
    }

    let t = transport(hello)
    let log = HookLog()
    let p = ScriptedPrompter([true, false])   // 移除：同意；退出：不同意
    let m = makeModel(t, log: log, prompter: p)
    await m.start()
    check("cca 有自己的同步服务器：这一页有它（有它的 cca 都有共享的开关），加入着 hub.example.com", m.hubAvailable && m.sharingAvailable
          && m.ownHub?.host == "hub.example.com:18443" && m.ownHub?.canInvite == true)
    check("还没打开这一页：不问服务器", hubRuns(t).isEmpty && m.hubDevices == nil)
    m.setMainVisible(true)
    m.selection = .sharing
    check("打开这一页：问一次服务器上有哪些设备（低优先级）", await waitUntil { m.hubDevices?.count == 2 }
          && hubRuns(t) == [["hub", "devices", "--json"]] && t.requests.first { $0.arguments.first == "hub" }?.background == true, "\(hubRuns(t))")
    check("这台在前；什么时候问到的照 cca 给的", m.hubDevices?.first?.me == true && m.hub.fetchedEpoch == 1794042000 && m.hubError == nil)

    mode.set("fail")
    m.refreshHubDevices(manual: true)
    check("问不到：上次问到的照样显示，旁边说 cca 给的原因", await waitUntil { m.hubError != nil && !m.hubLoading } && m.hubDevices?.count == 2
          && m.hubError?.contains("连不上") == true, m.hubError ?? "nil")
    check("点「刷新」问的不降优先级", t.requests.last { $0.arguments.first == "hub" }?.background == false)
    mode.set("ok")
    m.refreshHubDevices(manual: true)
    check("又问到了：原因去掉", await waitUntil { m.hubError == nil && !m.hubLoading })
    let queried = hubRuns(t).count
    m.refreshHubDevices(manual: false)
    m.refreshHubDevices(manual: true)   // 正在问的时候又要问（刚移除了设备）：不丢，问完再问一次
    check("正在问时又点了「刷新」：问完再问一次（第二次不降优先级）", await waitUntil { hubRuns(t).count == queried + 2 && !m.hubLoading }
          && t.requests.last { $0.arguments.first == "hub" }?.background == false, "\(hubRuns(t).count - queried)")
    m.refreshHubDevices(manual: false)
    m.refreshHubDevices(manual: false)
    _ = await waitUntil { hubRuns(t).count > queried + 2 && !m.hubLoading }
    try? await Task.sleep(nanoseconds: 100_000_000)
    check("后台的又碰上后台的：不再多问", hubRuns(t).count == queried + 3 && !m.hubLoading, "\(hubRuns(t).count - queried)")

    m.copyHubInvite()
    _ = await waitUntil { !log.copiedSecrets.isEmpty && !m.hubBusy && m.toast != nil }
    check("复制加入链接：只经 copySecret（去掉换行）；提示、读屏、普通的复制里都没有链接", log.copiedSecrets == [link] && log.copied.isEmpty
          && m.toast?.text.contains(link) == false && !log.announced.contains { $0.contains(link) }, m.toast?.text ?? "nil")
    let inv = t.requests.last { $0.arguments == ["hub", "invite"] }
    check("读加入链接：不是后台任务、不写文件", inv?.background == false && inv?.runsToCompletion == false)

    guard let carol = m.hubDevices?.first(where: { !$0.me }) else {
        check("演示数据里有另一台设备", false)
        m.shutdown()
        return
    }
    let asked = hubRuns(t).filter { $0 == ["hub", "devices", "--json"] }.count
    m.removeHubDevice(carol)
    check("移除时按钮先停用", m.hubBusy)
    _ = await waitUntil { !m.hubBusy && m.toast?.text == removed }
    let rm = t.requests.filter { $0.arguments.prefix(2) == ["hub", "remove"] }
    check("移除：先问同意（范围是设备指纹），同意后带着授权重跑一次；做完提示 cca 说的那句话",
          rm.count == 2 && rm[0].arguments == ["hub", "remove", carol.id] && rm[0].grants.isEmpty && rm[1].grants == ["hub_remove:\(carol.id)"]
          && m.toast?.text == removed, "\(rm.map(\.grants)) \(m.toast?.text ?? "nil")")
    check("移除以后再问一次服务器上有哪些设备", await waitUntil { hubRuns(t).filter { $0 == ["hub", "devices", "--json"] }.count > asked })
    m.leaveHub()
    _ = await waitUntil { !m.hubBusy && m.toast?.text == "没有同意，没有做任何改动" }
    check("退出：先问同意；不同意不重跑，什么都没改", t.requests.filter { $0.arguments == ["hub", "leave"] }.count == 1 && m.ownHub != nil
          && m.toast?.text == "没有同意，没有做任何改动")
    check("问同意的是 cca 给的（移除、退出各一次）", p.asked.map(\.item.rawValue) == ["hub_remove", "hub_leave"])
    m.shutdown()

    // 部署、从头部署、加入：都是提问会话（这里的替身不支持会话：只看开的是哪一个）
    let opens: [(String, @MainActor (AppModel) -> Void, [String])] = [
        ("部署", { $0.deployHub() }, ["hub", "deploy"]),
        ("从头部署", { $0.deployHub(reset: true) }, ["hub", "deploy", "--reset"]),
        ("加入", { $0.joinHub() }, ["hub", "join"]),
    ]
    for (label, act, want) in opens {
        let t = transport(hello)
        let m = makeModel(t)
        await m.start()
        act(m)
        check("\(label)：开提问会话 cca \(want.joined(separator: " "))", await waitUntil { !t.started.isEmpty } && t.started == [want], "\(t.started)")
        m.shutdown()
    }
}
