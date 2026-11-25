import Foundation

// 账号页的「设备」（PROTOCOL 3.4）：命令的样子；账号页看得见时才定时查（换账号马上查一次，看不见就停）；
// 查不成时上次查到的照样显示、旁边说原因；cca 说这个账号没有（can.devices 是 false）就不查。

private final class DevicesSwitch: @unchecked Sendable {
    private let lock = NSLock()
    private var _failing = false
    var failing: Bool {
        get { lock.withLock { _failing } }
        set { lock.withLock { _failing = newValue } }
    }
}

@MainActor func devicesTests() async {
    section("设备")
    let auto = CcaCommand.devices(.claude, key: "工作(bob@example.com)", auto: true)
    check("命令：cca devices KEY --json；定时刷新低优先级，点「刷新」的不降", auto.arguments == ["devices", "工作(bob@example.com)", "--json"]
          && auto.isBackground && !CcaCommand.devices(.claude, key: "k", auto: false).isBackground && !auto.runsToCompletion && auto.timeout == 30)
    check("ChatGPT 账号带前缀", CcaCommand.devices(.codex, key: "x", auto: false).arguments == ["codex", "devices", "x", "--json"])
    let all = CcaCommand.devicesAll(.agy)
    check("总览问全部账号的：不写账号、低优先级、不用跑完、一个工具一次一个", all.arguments == ["agy", "devices", "--json"]
          && CcaCommand.devicesAll(.claude).arguments == ["devices", "--json"] && all.isBackground && !all.runsToCompletion
          && all.timeout == 60 && all.gate == "devices-all:agy" && all.key == nil && all.tool == .agy)

    guard let hello = fixture("hello.json").map({ String(decoding: $0, as: UTF8.self) }),
          let state = fixture("state.json").map({ String(decoding: $0, as: UTF8.self) }),
          let devices = fixture("devices.json").map({ String(decoding: $0, as: UTF8.self) }) else {
        check("夹具", false)
        return
    }
    let sw = DevicesSwitch()
    let t = ScriptedTransport { request, _ in
        if request.arguments == ["_gui", "hello"] { return response(0, stdout: hello) }
        if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
        if request.arguments.first == "devices" {
            if sw.failing {
                return response(1, [.fail(FailEvent(code: .devicesUnavailable, message: "连不上设备统计的服务器（https://team.example.com:18445）"))])
            }
            return response(0, stdout: devices)
        }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    let m = makeModel(t, scheduler: Scheduler(devicesInterval: 3600))
    await m.start()
    // 总览一直在后台问全部账号的（devices --json，见 elsewhereTests）：这里只看账号页问的
    func asked() -> [[String]] { t.requests.map(\.arguments).filter { $0.first == "devices" && $0 != ["devices", "--json"] } }
    guard let bob = m.accounts(.claude).first(where: { $0.can.devices }), let carol = m.accounts(.claude).first(where: { !$0.can.devices && !$0.isAPI }) else {
        check("夹具里有 Team 给的、经它的服务器同步的账号（can.devices）", false)
        return
    }
    check("看不见的时候不查", !m.scheduler.devicesPollingRunning && asked().isEmpty)
    m.setMainVisible(true)
    m.selection = .account(bob.ref)
    check("打开 Team 给的账号的账号页：马上查一次，之后定时查", m.scheduler.devicesPollingRunning)
    check("查到了", await waitUntil { m.devices[bob.ref]?.account?.devices.count == 4 }, "\(asked())")
    check("问的是这个账号", asked().first == ["devices", bob.key, "--json"])
    let s = m.devices[bob.ref]
    check("这台 Mac 排第一，时间是这台 Mac 的", s?.account?.devices.first?.isMe == true && s?.doc?.me == "studio-mac" && s?.fetchedEpoch == 1794042000)

    sw.failing = true
    m.refreshDevices(bob.ref, manual: true)
    check("查不成：上次查到的照样显示，旁边说原因",
          await waitUntil { m.devices[bob.ref]?.error?.contains("连不上设备统计的服务器") == true }
          && m.devices[bob.ref]?.account?.devices.count == 4)
    sw.failing = false
    m.refreshDevices(bob.ref, manual: true)
    check("又查成了：原因去掉", await waitUntil { m.devices[bob.ref]?.error == nil && !m.devicesLoading.contains(bob.ref) })

    let n = asked().count
    m.selection = .account(carol.ref)
    check("cca 说没有「设备」的账号：不查，定时也停", !m.scheduler.devicesPollingRunning)
    m.refreshDevices(carol.ref, manual: true)
    check("点了也不查", !m.devicesLoading.contains(carol.ref) && asked().count == n)
    m.selection = .account(bob.ref)
    check("回到 Team 给的账号：马上再查一次", await waitUntil { asked().count == n + 1 })
    m.setMainVisible(false)
    check("窗口看不见了：停", !m.scheduler.devicesPollingRunning)
    m.selection = .office
    m.setMainVisible(true)
    check("别的页：不查", !m.scheduler.devicesPollingRunning)
    m.shutdown()
}

/// 总览：别的设备上在用的账号（cca devices --json 不写账号）也是在用，说是哪台；这台 Mac 自己的、闲置的、离线的不算；走开了不查
@MainActor func elsewhereTests() async {
    section("别的设备上在用")
    guard let hello = fixture("hello.json").map({ String(decoding: $0, as: UTF8.self) }),
          let state = fixture("state.json").map({ String(decoding: $0, as: UTF8.self) }),
          let raw = fixture("devices.json").map({ String(decoding: $0, as: UTF8.self) }) else {
        check("夹具", false)
        return
    }
    // alice-mini 从闲置改成在用
    let devices = raw.replacingOccurrences(of: #""state": "idle""#, with: #""state": "in_use""#)   // 夹具里只有 alice-mini 是闲置的
    check("夹具改好了", devices != raw)
    let t = ScriptedTransport { request, _ in
        if request.arguments == ["_gui", "hello"] { return response(0, stdout: hello) }
        if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
        if request.arguments == ["devices", "--json"] { return response(0, stdout: devices) }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    let m = makeModel(t, scheduler: Scheduler(devicesInterval: 3600))
    await m.start()
    func asked() -> Int { t.requests.filter { $0.arguments.suffix(2) == ["devices", "--json"] }.count }
    guard let bob = m.accounts(.claude).first(where: { $0.can.devices }) else {
        check("夹具里有经 Team 的服务器同步的账号", false)
        return
    }
    let got = await waitUntil { !m.elsewhereNames(bob.ref).isEmpty }
    check("连上 cca 就在后台查（只查有「设备」的工具）", m.scheduler.elsewherePollingRunning && got, "\(t.requests.map(\.arguments))")
    check("只算别的设备里在用的", m.elsewhereNames(bob.ref) == ["alice-mini"])
    check("过了一阵没再查成：不标了", m.elsewhereNames(bob.ref, now: Date().timeIntervalSince1970 + AppModel.elsewhereFresh + 1).isEmpty)
    let e = m.office(now: Date().timeIntervalSince1970).people.first { $0.account.ref == bob.ref }
    check("总览：这台 Mac 上没在用也是在用，说是哪台", e?.mood == .working && e?.isWorking == bob.isWorking
          && (bob.isWorking || Texts.mood(e!, appLabel: "Claude App", now: 0) == "在「alice-mini」上在用"),
          "\(String(describing: e.map { Texts.mood($0, appLabel: "Claude App", now: 0) }))")
    m.shutdown()

    let acct = m.accounts(.claude).first { !$0.isWorking && !$0.isAPI } ?? bob
    let two = Employee.make(acct, usage: nil, live: nil, now: 0, elsewhere: ["一个很长很长很长很长的设备名字", "b"])
    check("几台：说第一台和一共几台，短写法截短", Texts.mood(two, appLabel: "", now: 0).hasPrefix("在「一个很长很长很长很长的设备名字」等 2 台设备上在用")
          && Texts.mood(two, appLabel: "", now: 0, brief: true).hasPrefix("在「一个很长很长很长很长的设备…」等 2 台设备上在用"))
    let none = Employee.make(acct, usage: nil, live: nil, now: 0)
    check("别的设备上没在用：照旧", none.elsewhere.isEmpty && (acct.isWorking || none.mood != .working))
}
