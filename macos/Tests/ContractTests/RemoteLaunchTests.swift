import Foundation

// 远程启动（PROTOCOL 9.4）：模板的命令和文档、App 里查模板、新建 / 修改 / 删掉模板（参数那一行原样交给 cca）、「允许远程启动」的开关；
// 用配对过的另一台机器上的模板在那台机器上新建会话（PROTOCOL 9.5）：`peer_launchers`（3.7.10 起带能挑的账号）、`cca remote launch … --json`、
// 弹层（挑模板、账号）、起好了接入。

/// 测试里 cca 那一头：模板列表（可以换）、查模板查不成、存模板时 cca 说的不行（线程安全）
private final class LaunchScript: @unchecked Sendable {
    private let lock = NSLock()
    private var _templates: String
    private var _failList = false
    private var _saveError: String?
    init(templates: String) { _templates = templates }
    var templates: String {
        get { lock.withLock { _templates } }
        set { lock.withLock { _templates = newValue } }
    }
    var failList: Bool {
        get { lock.withLock { _failList } }
        set { lock.withLock { _failList = newValue } }
    }
    var saveError: String? {
        get { lock.withLock { _saveError } }
        set { lock.withLock { _saveError = newValue } }
    }
}

/// 一台配对过的机器（夹具里没有的样子）
private func remotePeer(_ json: String) -> RemotePeer? {
    (try? DocumentDecoder.decode(RemoteDoc.self, from: Data((#"{"version":1,"kind":"remote","sessions":[],"peers":["# + json + "]}").utf8)))?.peers.first
}

private func launchUpdated(_ text: String) -> CcaResponse {
    response(0, [.result(ResultEvent(code: .remoteUpdated, route: nil, node: nil, port: nil, message: text))])
}

func remoteLaunchProtocolTests() {
    section("远程启动：命令和文档")
    let pid = String(repeating: "e4", count: 16)
    let list = CcaCommand.remoteTemplates
    check("查模板：cca remote template list --json；只读，不和改东西的排队", list.arguments == ["remote", "template", "list", "--json"] && list.gate == nil
          && !list.runsToCompletion && list.tool == nil && list.key == nil)
    let add = CcaCommand.remoteTemplateSave(name: "网站", tool: .claude, dir: "/Users/me/projects/site", argsLine: "--model opus 'a b'", replace: nil)
    check("存模板：都给全（cca 不在终端里问），参数那一行原样放在 --args-line 后面（cca 拆）；账号不在模板里：不带 --account",
          add.arguments == ["remote", "template", "add", "--name", "网站", "--tool", "claude",
                            "--dir", "/Users/me/projects/site", "--args-line", "--model opus 'a b'"], "\(add.arguments)")
    let replace = CcaCommand.remoteTemplateSave(name: "队列", tool: .codex, dir: "/Users/me/srv/jobs", argsLine: "", replace: "5e6f7a8b")
    check("改模板：带 --replace <编号>；没有参数也有 --args-line（空的）",
          replace.arguments == ["remote", "template", "add", "--name", "队列", "--tool", "codex",
                                "--dir", "/Users/me/srv/jobs", "--replace", "5e6f7a8b", "--args-line", ""], "\(replace.arguments)")
    check("参数那一行是 --、以 - 开头的也原样是 --args-line 的值",
          CcaCommand.remoteTemplateSave(name: "n", tool: .claude, dir: "/d", argsLine: "-- -x", replace: nil).arguments.suffix(2) == ["--args-line", "-- -x"])
    check("删模板：按编号", CcaCommand.remoteTemplateRemove(id: "1a2b3c4d").arguments == ["remote", "template", "rm", "1a2b3c4d"])
    check("允许、不允许远程启动：按配对的 id", CcaCommand.remoteLaunch(peer: pid, on: true).arguments == ["remote", "allow", pid, "launch", "on"]
          && CcaCommand.remoteLaunch(peer: pid, on: false).arguments == ["remote", "allow", pid, "launch", "off"])
    let changes = [add, replace, CcaCommand.remoteTemplateRemove(id: "1a2b3c4d"), CcaCommand.remoteLaunch(peer: pid, on: true)]
    check("改模板、改远程启动和改配对表的几条一个接一个；都不是哪个账号的",
          changes.allSatisfy { $0.gate != nil && $0.gate == CcaCommand.remoteUnpair(peer: pid).gate && !$0.runsToCompletion && $0.tool == nil && $0.key == nil })
    check("同意项认识", ConsentItem.remoteLaunch.isKnown && ConsentItem.remoteLaunch.rawValue == "remote_launch")
    let start = CcaCommand.remoteStart(peer: pid, template: "2b2b2b2b", digest: "0a1b2c3d4e5f6a7b", cols: 132, rows: 40, line: "先跑一遍测试",
                                       account: "a1a1a1a1a1a1a1a1")
    check("在别的机器上新建会话：cca remote launch <配对的 id> <模板的编号> --digest <摘要> --size 列x行 --account 挑的账号 --line 开场白 --json",
          start.arguments == ["remote", "launch", pid, "2b2b2b2b", "--digest", "0a1b2c3d4e5f6a7b", "--size", "132x40", "--account", "a1a1a1a1a1a1a1a1",
                              "--line", "先跑一遍测试", "--json"], "\(start.arguments)")
    let bare = CcaCommand.remoteStart(peer: pid, template: "2b2b2b2b", digest: "0a1b2c3d4e5f6a7b", cols: 120, rows: 36, line: nil)
    check("没有开场白、不用挑账号（那台机器上的 cca 是旧版）：不带 --line、--account；一个接一个，不和改配对表的排队，不是哪个账号的；要等那台机器起好、登记好（超时比问一次服务器长得多）",
          bare.arguments == ["remote", "launch", pid, "2b2b2b2b", "--digest", "0a1b2c3d4e5f6a7b", "--size", "120x36", "--json"]
          && bare.gate != nil && bare.gate != CcaCommand.remoteUnpair(peer: pid).gate && !bare.runsToCompletion && bare.tool == nil && bare.key == nil
          && bare.timeout >= 90 && !bare.isBackground, "\(bare.arguments)")
    check("hello 的 remote_launch_machines；3.7.6 的 cca 没有",
          (try? DocumentDecoder.decode(HelloDoc.self, from: Data(#"{"version":1,"kind":"hello","api":1,"remote":1,"remote_launch":1,"remote_launch_machines":1}"#.utf8)))?.remoteLaunchMachines == 1
          && (try? DocumentDecoder.decode(HelloDoc.self, from: Data(#"{"version":1,"kind":"hello","api":1,"remote":1,"remote_launch":1}"#.utf8)))?.remoteLaunchMachines == nil)
    check("hello 的 remote_launch；旧版 cca 没有",
          (try? DocumentDecoder.decode(HelloDoc.self, from: Data(#"{"version":1,"kind":"hello","api":1,"remote":1,"remote_launch":1}"#.utf8)))?.remoteLaunch == 1
          && (try? DocumentDecoder.decode(HelloDoc.self, from: Data(#"{"version":1,"kind":"hello","api":1,"remote":1}"#.utf8)))?.remoteLaunch == nil)

    // 模板的文档
    guard let d = fixture("remote_templates.json"), let doc = try? DocumentDecoder.decode(RemoteTemplatesDoc.self, from: d) else {
        check("夹具 remote_templates.json", false)
        return
    }
    let t = doc.templates
    check("模板：编号、名字、工具、文件夹、参数（账号不在模板里）", t.map(\.id) == ["1a2b3c4d", "5e6f7a8b", "9c0d1e2f"] && t.map(\.tool) == [.claude, .codex, .claude]
          && t[0].name == "网站：修 bug" && t[0].dir == "/Users/me/projects/site"
          && t[0].args == ["--model", "opus", "--append-system-prompt", "说中文，改完跑一遍测试"] && t[1].args.isEmpty && t.allSatisfy(\.line))
    check("用不了的说原因：文件夹不在了", t[2].problem == .noDir && t.prefix(2).allSatisfy { $0.problem == nil })
    check("参数拼成的那一行：cca 给的",
          t.map(\.argsLine) == ["--model opus --append-system-prompt '说中文，改完跑一遍测试'", "", "--permission-mode plan"], "\(t.map(\.argsLine))")
    check("启动器：在跑、几个模板、几台手机", doc.launcher == RemoteLauncher(running: true, templates: 3, peers: 1))
    check("按编号找", doc.template("5e6f7a8b")?.name == "任务队列" && doc.template("nope") == nil)
    let odd = try? DocumentDecoder.decode(RemoteTemplatesDoc.self, from: Data(#"""
        {"version":1,"kind":"remote_templates","templates":[{"id":"aa","name":"x","tool":"zz_new","account":"a@example.com","dir":"/x",
         "args":["--a",1,"b"],"problem":"zz_new"},{"name":"没有编号"}]}
        """#.utf8))
    check("没有编号的丢掉；不认识的工具、原因留着；参数里不是字符串的丢掉；没有 launcher 是 nil",
          odd?.templates.count == 1 && odd?.templates.first?.tool?.isKnown == false && odd?.templates.first?.problem?.isKnown == false
          && odd?.templates.first?.args == ["--a", "b"] && odd?.templates.first?.line == false && odd?.launcher == nil)
    let older = try? DocumentDecoder.decode(RemoteTemplatesDoc.self, from: Data(#"""
        {"version":1,"kind":"remote_templates","templates":[{"id":"0a0b0c0d","name":"x","tool":"claude","account":"a@example.com",
         "account_name":"a","dir":"/x","args":["--append-system-prompt","it's 中文",""]}]}
        """#.utf8))
    check("3.7.9 及以前的 cca 写的（模板带着账号、缺了 args_line）：照样读；参数照 shell 的写法拼（要引的才引，不然改模板时参数会被清空）",
          older?.templates.first?.argsLine == #"--append-system-prompt 'it'\''s 中文' ''"# && older?.templates.first?.dir == "/x",
          older?.templates.first?.argsLine ?? "nil")

    // 列表里的：哪台手机能远程启动、启动器
    guard let ld = fixture("remote_list.json"), let rl = try? DocumentDecoder.decode(RemoteDoc.self, from: ld) else {
        check("夹具 remote_list.json", false)
        return
    }
    check("列表：哪台手机能远程启动、启动器", rl.peers.map(\.launch) == [false, false, true, false] && rl.launcher == RemoteLauncher(running: true, templates: 3, peers: 1))
    let old = try? DocumentDecoder.decode(RemoteDoc.self, from: Data(#"{"version":1,"kind":"remote","sessions":[],"peers":[{"id":"aa","web":true,"incoming":"control"}]}"#.utf8))
    check("旧版 cca 没有这两项：不算能远程启动，没有启动器", old?.peers.first?.launch == false && old?.launcher == nil && old?.peerLaunchers.isEmpty == true)

    // 别的机器上允许这台 Mac 远程启动的（9.5）
    let pl = rl.peerLaunchers
    check("peer_launchers：哪个配对、机器的名字、连没连着、模板（编号、摘要、名字、工具、账号、那台机器上的名字、文件夹、参数、能不能带开场白）",
          pl.map(\.machine) == ["office-linux", "home-mini"] && pl.map(\.online) == [true, false] && pl[0].peer == "b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1"
          && pl[0].templates.map(\.id) == ["2b2b2b2b", "4d4d4d4d"] && pl[0].templates.map(\.line) == [true, false] && pl[0].templates.map(\.tool) == [.claude, .codex]
          && pl[0].templates[0].digest == "0a1b2c3d4e5f6a7b" && pl[0].templates[0].account == "启动时选账号" && pl[1].templates[0].account == "个人"
          && pl[0].templates[0].dir == "~/srv/api" && pl[0].templates[0].args == "--model opus" && pl[1].more == 2 && pl[0].more == 0
          && rl.peerLauncher("c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2")?.machine == "home-mini" && rl.peerLauncher("zz") == nil)
    check("peer_launchers 的 accounts（3.7.10 起：起会话时挑）：编号、工具、这台 Mac 上的名字、那台机器上的名字；那台机器上的 cca 是旧版时是 null",
          pl[0].accounts?.map(\.id) == ["a1a1a1a1a1a1a1a1", "a2a2a2a2a2a2a2a2", "a3a3a3a3a3a3a3a3"] && pl[0].accounts?.map(\.tool) == [.claude, .claude, .codex]
          && pl[0].accounts?.first?.name == "工作" && pl[0].accounts?.first?.hostName == "主力" && pl[1].accounts == nil)
    check("按模板的工具挑：claude 的模板两个、codex 的一个；那台机器上的 cca 是旧版：nil（模板用它自己的账号）",
          pl[0].accounts(for: pl[0].templates[0])?.map(\.id) == ["a1a1a1a1a1a1a1a1", "a2a2a2a2a2a2a2a2"]
          && pl[0].accounts(for: pl[0].templates[1])?.map(\.id) == ["a3a3a3a3a3a3a3a3"] && pl[1].accounts(for: pl[1].templates[0]) == nil)
    let oddL = try? DocumentDecoder.decode(RemoteDoc.self, from: Data(#"""
        {"version":1,"kind":"remote","sessions":[],"peers":[],"peer_launchers":[{"peer":"b1"},{"id":"aa","peer":"b1","state":"zz_new","more":-3,
         "templates":[{"id":"0a0b0c0d","name":"没有摘要"},{"id":"1a1b1c1d","digest":"0011223344556677","tool":"zz_new"}],
         "accounts":[{"tool":"claude","name":"没有编号"},{"id":"b1b2b3b4b5b6b7b8","tool":"zz_new","name":"新工具的"},{"id":"c1c2c3c4c5c6c7c8","name":"没有工具"},
          {"id":"d1d2d3d4d5d6d7d8","tool":"claude","name":"x"}]}]}
        """#.utf8))
    let oddA = oddL?.peerLaunchers.first
    check("没有编号、配对的启动器丢掉；没有摘要的模板丢掉；不认识的状态、工具留着（不算连着）；more 不是负的",
          oddL?.peerLaunchers.count == 1 && oddA?.online == false && oddA?.more == 0
          && oddA?.templates.map(\.id) == ["1a1b1c1d"] && oddA?.templates.first?.tool?.isKnown == false && oddA?.templates.first?.line == false)
    check("能挑的账号：没有编号的丢掉；没有那台机器上的名字时就是名字；没有工具的哪个模板都挑不了，不认识的工具只给同一个工具的模板",
          oddA?.accounts?.map(\.id) == ["b1b2b3b4b5b6b7b8", "c1c2c3c4c5c6c7c8", "d1d2d3d4d5d6d7d8"] && oddA?.accounts?.first?.hostName == "新工具的"
          && oddA.flatMap { a in a.templates.first.flatMap { a.accounts(for: $0) } }?.map(\.id) == ["b1b2b3b4b5b6b7b8"])
    let sid = String(repeating: "f7", count: 16)
    let started = try? DocumentDecoder.decode(RemoteLaunchStartDoc.self, from: Data(#"""
        {"version":1,"kind":"remote_launch","id":"\#(sid)","session":{"id":"\#(sid)","short":"f7f7f7f7","where":"peer","peer":"b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1",
         "machine":"office-linux","tool":"claude","name":"工作","host_name":"主力","cwd":"~/srv/api","since_epoch":1794042000,"viewers":0,
         "local_terminal":null,"state":"ok","access":"control","team":"5d0c2a9be17f4c3e8a6b1d2f3e4c5a6b"}}
        """#.utf8))
    check("remote launch --json：新会话的编号、它在列表里的那一项（接得上）", started?.id == sid && started?.session?.id == sid
          && started?.session?.place == .peer && started?.session?.attachable == true && started?.session?.machine == "office-linux")
    let unlisted = try? DocumentDecoder.decode(RemoteLaunchStartDoc.self, from: Data(#"{"version":1,"kind":"remote_launch","id":"\#(sid)","session":null}"#.utf8))
    check("还没登记好：session 是 null；没有编号的不算", unlisted?.id == sid && unlisted?.session == nil
          && (try? DocumentDecoder.decode(RemoteLaunchStartDoc.self, from: Data(#"{"version":1,"kind":"remote_launch","session":null}"#.utf8))) == nil
          && (try? DocumentDecoder.decode(RemoteLaunchStartDoc.self, from: Data(#"{"version":1,"kind":"remote","id":"\#(sid)"}"#.utf8))) == nil)
    check("另一台机器上的模板的第二行：文件夹、参数；那台机器上的 cca 是旧版时前面还有模板的账号",
          Texts.remotePeerTemplateDetail(pl[0].templates[0], withAccount: false) == "~/srv/api · --model opus"
          && Texts.remotePeerTemplateDetail(pl[1].templates[0]) == "个人 · ~/notes")
    check("能挑的账号：这台 Mac 上的名字，和那台机器上叫的不一样时括号里说一声",
          pl[0].accounts?.map(Texts.remotePeerAccount) == ["工作（那台机器上叫「主力」）", "carol", "dave"], "\(pl[0].accounts?.map(Texts.remotePeerAccount) ?? [])")

    // 文字
    check("模板的第二行：文件夹、参数（照 shell 的写法）",
          Texts.remoteTemplateDetail(t[0], dir: "~/projects/site") == "~/projects/site · --model opus --append-system-prompt '说中文，改完跑一遍测试'"
          && Texts.remoteTemplateDetail(t[1], dir: "~/srv/jobs") == "~/srv/jobs"
          && Texts.remoteTemplateDetail(t[2], dir: "~/notes") == "~/notes · --permission-mode plan", Texts.remoteTemplateDetail(t[0], dir: "~/projects/site"))
    check("用不了的原因：短的标在名字后面，不认识的也有一句", Texts.remoteTemplateProblem(.noAccount) == "没有账号" && Texts.remoteTemplateProblem(.noRelay) == "不经中转"
          && Texts.remoteTemplateProblem(RemoteTemplateProblem(rawValue: "zz_new")) == "暂不可用"
          && RemoteTemplateProblem.allKnown.allSatisfy { !Texts.remoteTemplateProblemHelp($0).isEmpty && Texts.remoteTemplateProblem($0) != "现在用不了" })
}

@MainActor func remoteLaunchModelTests() async {
    section("远程启动：模板和「允许远程启动」")
    guard let helloData = fixture("hello.json"), let state = fixture("state.json").map({ String(decoding: $0, as: UTF8.self) }),
          let list = fixture("remote_list.json").map({ String(decoding: $0, as: UTF8.self) }),
          let templates = fixture("remote_templates.json").map({ String(decoding: $0, as: UTF8.self) }) else {
        check("夹具", false)
        return
    }
    let hello = String(decoding: helloData, as: UTF8.self)

    // 旧版 cca（hello 没有 remote_launch）：有远程会话，没有远程启动
    do {
        let old = hello.replacingOccurrences(of: #""remote_launch": 1,"#, with: "")
        let t = ScriptedTransport { request, _ in
            if request.arguments == ["_gui", "hello"] { return response(0, stdout: old) }
            if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
            if request.arguments.prefix(2) == ["remote", "list"] { return response(0, stdout: list) }
            return response(0, stdout: SampleDocs.clientUpdates)
        }
        let m = makeModel(t)
        await m.start()
        m.setMainVisible(true)
        m.selection = .remoteSessions
        let got = await waitUntil { m.remote.doc != nil && !m.remoteLoading }
        m.refreshRemote(manual: true)
        m.refreshRemoteTemplates()
        m.editRemoteTemplate()
        if let phone = m.remote.doc?.peers.first(where: \.web) { m.setRemoteLaunch(phone, false) }
        try? await Task.sleep(nanoseconds: 100_000_000)
        check("旧版 cca（hello 没有 remote_launch）：不查模板、不弹模板、不调远程启动的命令、不说启动器",
              got && old != hello && m.remoteAvailable && !m.remoteLaunchAvailable && m.remoteTemplates.doc == nil && m.sheet == nil
              && m.remoteLauncherRunning == nil && !t.requests.contains { $0.arguments.prefix(2) == ["remote", "template"] || $0.arguments.contains("launch") })
        m.shutdown()
    }

    // 3.7.6 的 cca（hello 有 remote_launch、没有 remote_launch_machines）：只有手机能远程启动，没有「新建会话…」
    do {
        let older = hello.replacingOccurrences(of: #""remote_launch_machines": 1,"#, with: "")
        let t = ScriptedTransport { request, _ in
            let a = request.arguments
            if a == ["_gui", "hello"] { return response(0, stdout: older) }
            if a == ["_gui", "state"] { return response(0, stdout: state) }
            if a.prefix(2) == ["remote", "list"] { return response(0, stdout: list) }
            if a == ["remote", "template", "list", "--json"] { return response(0, stdout: templates) }
            return response(0, stdout: SampleDocs.clientUpdates)
        }
        let m = makeModel(t)
        await m.start()
        m.setMainVisible(true)
        m.selection = .remoteSessions
        let got = await waitUntil { m.remote.doc != nil && !m.remoteLoading }
        let mini = m.remote.doc?.peers.first { !$0.web && $0.incoming == .control }
        if let mini { m.setRemoteLaunch(mini, true) }
        if let l = m.remote.doc?.peerLaunchers.first { m.newRemoteSession(on: l) }
        try? await Task.sleep(nanoseconds: 100_000_000)
        check("3.7.6 的 cca（hello 没有 remote_launch_machines）：配对过的电脑不能开「允许远程启动」，不给「新建会话…」，不调这两种命令",
              got && older != hello && m.remoteLaunchAvailable && !m.remoteLaunchMachinesAvailable && m.remotePeerLaunchers.isEmpty
              && mini.map { m.remoteLaunchBlocked($0) == "只有手机能远程启动会话" && !m.remoteLaunchOffered($0) } == true && m.sheet == nil
              && !t.requests.contains { $0.arguments.contains("launch") })
        m.shutdown()
    }

    let box = LaunchScript(templates: templates)
    let t = ScriptedTransport { request, _ in
        let a = request.arguments
        if a == ["_gui", "hello"] { return response(0, stdout: hello) }
        if a == ["_gui", "state"] { return response(0, stdout: state) }
        if a.prefix(2) == ["remote", "list"] { return response(0, stdout: list) }
        if a == ["remote", "template", "list", "--json"] {
            return box.failList ? response(1, stderr: "读不了模板\n") : response(0, stdout: box.templates)
        }
        if a.prefix(3) == ["remote", "template", "add"], a.count > 4 {
            if let e = box.saveError { return response(1, [.fail(FailEvent(code: .error, message: e))], stderr: e) }
            return launchUpdated("已存好模板「\(a[4])」")
        }
        if a.prefix(3) == ["remote", "template", "rm"] { return launchUpdated("已删掉模板「旧笔记」") }
        if a.count == 5, a.prefix(2) == ["remote", "allow"], a[3] == "launch" {
            if a[4] == "on", !request.grants.contains("remote_launch:" + a[2]) {
                return response(1, [consentEvent("remote_launch", a[2], yes: false), .fail(FailEvent(code: .error, message: "要先同意"))])
            }
            return launchUpdated(a[4] == "on" ? "「备用 iPhone」可以用这台机器上的模板新建会话了" : "「我的 iPhone」不能再用模板新建会话了")
        }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    let prompter = ScriptedPrompter([true])
    let scheduler = Scheduler()
    scheduler.remoteInterval = 3600
    let m = makeModel(t, prompter: prompter, scheduler: scheduler)
    await m.start()
    func asked() -> [[String]] { t.requests.map(\.arguments).filter { $0.first == "remote" } }
    func templateLists() -> Int { asked().filter { $0 == ["remote", "template", "list", "--json"] }.count }
    func settled() -> Bool { !m.remoteBusy && !m.remoteLoading && !m.remoteTemplatesLoading }
    check("cca 有远程启动（hello.remote_launch）", m.remoteLaunchAvailable)
    check("这一页看不见时不查模板", templateLists() == 0 && m.remoteTemplates.doc == nil)
    m.setMainVisible(true)
    m.selection = .remoteSessions
    check("打开这一页：模板也查一次（cca remote template list --json）",
          await waitUntil { m.remoteTemplates.doc?.templates.count == 3 && m.remote.doc != nil && settled() }, "\(asked())")
    check("列表里说的模板个数对得上：不再查", templateLists() == 1, "\(asked())")
    check("有允许远程启动的手机：说启动器在跑", m.remoteLauncherRunning == true && m.remoteLauncher?.peers == 1)

    // 列表里说的模板个数和手里的对不上（在终端里加过、删过）：跟着列表再查一次模板
    box.templates = templates.replacingOccurrences(of: #""name": "旧笔记""#, with: #""name": "旧笔记（改过）""#)
    m.refreshRemote(manual: false)
    _ = await waitUntil { settled() }
    check("对得上时定时查列表不查模板", templateLists() == 1 && m.remoteTemplates.doc?.template("9c0d1e2f")?.name == "旧笔记", "\(asked())")
    box.templates = #"{"version":1,"kind":"remote_templates","templates":[],"launcher":{"running":true,"templates":0,"peers":1}}"#
    m.refreshRemoteTemplates()
    _ = await waitUntil { settled() && m.remoteTemplates.doc?.templates.isEmpty == true }
    m.refreshRemote(manual: false)
    check("对不上：再查一次模板", await waitUntil { settled() && templateLists() == 3 }, "\(asked())")
    box.templates = templates
    box.failList = true
    m.refreshRemote(manual: true)
    check("查模板查不成：上次查到的照样显示，旁边说原因", await waitUntil { settled() && m.remoteTemplates.error != nil }
          && m.remoteTemplates.doc?.templates.isEmpty == true, m.remoteTemplates.error ?? "nil")
    let failed = templateLists()
    m.refreshRemote(manual: false)
    _ = await waitUntil { settled() }
    check("上次没查成：定时查列表时不跟着一遍遍查模板", templateLists() == failed, "\(asked())")
    box.failList = false
    m.refreshRemote(manual: true)
    check("点「刷新」：又查成了，原因去掉", await waitUntil { settled() && m.remoteTemplates.error == nil && m.remoteTemplates.doc?.templates.count == 3 })

    // 「允许远程启动」
    guard let doc = m.remote.doc, let phone = doc.peers.first(where: \.web), let mini = doc.peers.first(where: { !$0.web && $0.incoming == .control }),
          let spare = remotePeer(#"{"id":"f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5","name":"备用 iPhone","incoming":"control","shown":true,"web":true}"#),
          let viewer = remotePeer(#"{"id":"a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6","name":"只看的 iPhone","incoming":"view","shown":true,"web":true}"#) else {
        check("夹具里有手机、别的机器", false)
        return
    }
    check("开关：开着的、能开的不拦；只让看的说原因（cca 也这么查）；cca 有 remote_launch_machines：配对过的电脑也能开",
          phone.launch && m.remoteLaunchBlocked(phone) == nil && m.remoteLaunchBlocked(spare) == nil
          && m.remoteLaunchBlocked(mini) == nil && m.remoteLaunchOffered(mini) && m.remoteLaunchBlocked(viewer)?.contains("看和操作") == true)
    var n = asked().count
    m.setRemoteLaunch(phone, true)
    m.setRemoteLaunch(viewer, true)
    check("没变的、拦着的：不调命令", asked().count == n && !m.remoteBusy)
    m.setRemoteLaunch(phone, false)
    check("关掉：cca remote allow <配对的 id> launch off，不用同意；做完再查一次列表和模板",
          await waitUntil { settled() && asked().count >= n + 3 } && asked()[n] == ["remote", "allow", phone.id, "launch", "off"] && prompter.asked.isEmpty
          && asked()[(n + 1)...].contains(["remote", "list", "--json"]) && asked()[(n + 1)...].contains(["remote", "template", "list", "--json"]),
          "\(asked().suffix(4))")
    check("显示 cca 说的那句话", m.toast?.text.contains("不能再用模板新建会话了") == true, "\(String(describing: m.toast))")
    let on = ["remote", "allow", spare.id, "launch", "on"]
    m.setRemoteLaunch(spare, true)
    check("打开：cca 先要同意（范围是配对的 id），同意了带着授权重跑",
          await waitUntil { settled() && t.requests.filter { $0.arguments == on }.count == 2 }
          && t.requests.filter { $0.arguments == on }.map(\.grants) == [[], ["remote_launch:" + spare.id]]
          && prompter.asked.map(\.grant) == ["remote_launch:" + spare.id], "\(t.requests.filter { $0.arguments == on }.map(\.grants))")
    check("打开了：显示 cca 说的那句话", m.toast?.text.contains("可以用这台机器上的模板新建会话了") == true, "\(String(describing: m.toast))")

    // 新建模板
    m.editRemoteTemplate()
    guard case .remoteTemplate(let draft)? = m.sheet else {
        check("新建模板：弹层", false)
        return
    }
    check("新建：选上现在的工具（账号不在模板里：起会话时挑）；名字、文件夹空着不能存",
          draft.original == nil && draft.tool == .claude && draft.name.isEmpty && draft.args.isEmpty
          && !draft.canSave && m.remoteTemplateTools == [.claude, .codex])
    m.setRemoteTemplateTool(draft, .codex)
    check("换工具", draft.tool == .codex)
    m.setRemoteTemplateTool(draft, .claude)
    draft.name = "  网站：写文档  "
    m.hooks.chooseTemplateDirectory = { _ in "/Users/me/projects/docs" }
    m.chooseRemoteTemplateDirectory(draft)
    check("选文件夹", await waitUntil { draft.dir == "/Users/me/projects/docs" } && draft.canSave)
    draft.args = "--model opus --append-system-prompt '说中文"
    box.saveError = "参数里的引号没有配对：拆不开"
    m.saveRemoteTemplate(draft)
    check("参数那一行原样交给 cca 拆；拆不开：cca 说的原因写在弹层里，接着改",
          await waitUntil { !draft.saving && draft.error != nil } && asked().last?.suffix(2) == ["--args-line", "--model opus --append-system-prompt '说中文"]
          && draft.error == "参数里的引号没有配对：拆不开" && m.sheet == .remoteTemplate(draft), "\(asked().suffix(2)) \(draft.error ?? "nil")")
    draft.args = "--model opus --append-system-prompt '说中文，别改测试'"
    box.saveError = "已经有叫「网站：写文档」的模板了"
    m.saveRemoteTemplate(draft)
    let added = ["remote", "template", "add", "--name", "网站：写文档", "--tool", "claude",
                 "--dir", "/Users/me/projects/docs", "--args-line", "--model opus --append-system-prompt '说中文，别改测试'"]
    check("存：名字去掉两头的空白，参数那一行原样放在 --args-line 后面；cca 说不行：原因写在弹层里，接着改",
          await waitUntil { !draft.saving && draft.error == "已经有叫「网站：写文档」的模板了" } && asked().last == added
          && m.sheet == .remoteTemplate(draft), "\(asked().suffix(2)) \(draft.error ?? "nil")")
    box.saveError = nil
    let lists = templateLists()
    m.saveRemoteTemplate(draft)
    check("存好了：弹层关上，说 cca 说的那句话，再查一次列表和模板",
          await waitUntil { m.sheet == nil && !draft.saving && settled() && templateLists() > lists } && draft.error == nil
          && m.toast?.text == "已存好模板「网站：写文档」", "\(asked().suffix(3)) \(String(describing: m.toast))")

    // 修改模板
    guard let site = m.remoteTemplates.doc?.template("1a2b3c4d"), let gone = m.remoteTemplates.doc?.template("9c0d1e2f") else {
        check("查到的模板", false)
        return
    }
    m.editRemoteTemplate(site)
    guard case .remoteTemplate(let e)? = m.sheet else {
        check("修改模板：弹层", false)
        return
    }
    check("修改：填好原来的；参数是 cca 拼好的那一行（args_line）",
          e.original?.id == "1a2b3c4d" && e.name == "网站：修 bug" && e.tool == .claude
          && e.dir == "/Users/me/projects/site" && e.args == "--model opus --append-system-prompt '说中文，改完跑一遍测试'" && e.canSave, e.args)
    e.args += " --verbose"
    m.saveRemoteTemplate(e)
    let replaced = ["remote", "template", "add", "--name", "网站：修 bug", "--tool", "claude",
                    "--dir", "/Users/me/projects/site", "--replace", "1a2b3c4d", "--args-line",
                    "--model opus --append-system-prompt '说中文，改完跑一遍测试' --verbose"]
    check("存修改：带 --replace <编号>", await waitUntil { m.sheet == nil && !e.saving && settled() } && asked().contains(replaced), "\(asked().suffix(3))")
    m.editRemoteTemplate(gone)
    if case .remoteTemplate(let g)? = m.sheet {
        check("修改用不了的模板（文件夹不在了）：照样填好原来的，能存（cca 校验，说原因）",
              g.tool == .claude && g.original?.id == "9c0d1e2f" && g.dir == "/Users/me/notes" && g.args == "--permission-mode plan" && g.canSave)
    } else {
        check("修改用不了的模板：弹层", false)
    }
    m.dismissSheet()
    let agyDoc = try? DocumentDecoder.decode(RemoteTemplatesDoc.self, from: Data(
        #"{"version":1,"kind":"remote_templates","templates":[{"id":"0a0b0c0d","name":"agy","tool":"agy","account":"carol@example.com","dir":"/x","args":[]}]}"#.utf8))
    if let agy = agyDoc?.templates.first {
        m.editRemoteTemplate(agy)
        if case .remoteTemplate(let g)? = m.sheet {
            check("模板的工具没启用：换成能选的", g.tool == .claude && g.dir == "/x")
        } else {
            check("模板的工具没启用：弹层", false)
        }
        m.dismissSheet()
    }

    // 删模板
    n = asked().count
    m.removeRemoteTemplate(gone)
    check("删模板：cca remote template rm <编号>，做完说一句、再查一次",
          await waitUntil { settled() && asked().count >= n + 3 } && asked()[n] == ["remote", "template", "rm", "9c0d1e2f"]
          && asked()[(n + 1)...].contains(["remote", "template", "list", "--json"]) && m.toast?.text == "已删掉模板「旧笔记」", "\(asked().suffix(3))")
    m.shutdown()
}

/// 测试里 cca 那一头的 `remote launch`：起不成时说的话、起好了给不给新会话（线程安全）
private final class StartScript: @unchecked Sendable {
    private let lock = NSLock()
    private var _fail: String?
    private var _unlisted = false
    var fail: String? {
        get { lock.withLock { _fail } }
        set { lock.withLock { _fail = newValue } }
    }
    var unlisted: Bool {
        get { lock.withLock { _unlisted } }
        set { lock.withLock { _unlisted = newValue } }
    }
}

@MainActor func remoteLaunchGuestTests() async {
    section("在配对过的另一台机器上新建会话（9.5）")
    guard let helloData = fixture("hello.json"), let state = fixture("state.json").map({ String(decoding: $0, as: UTF8.self) }),
          let list = fixture("remote_list.json").map({ String(decoding: $0, as: UTF8.self) }),
          let templates = fixture("remote_templates.json").map({ String(decoding: $0, as: UTF8.self) }) else {
        check("夹具", false)
        return
    }
    let hello = String(decoding: helloData, as: UTF8.self)
    let sid = String(repeating: "f7", count: 16)
    let session = #"{"id":"\#(sid)","short":"f7f7f7f7","where":"peer","peer":"b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1","machine":"office-linux","tool":"claude","name":"工作","host_name":"主力","cwd":"~/srv/api","since_epoch":1794042000,"viewers":0,"local_terminal":null,"state":"ok","access":"control","team":"5d0c2a9be17f4c3e8a6b1d2f3e4c5a6b"}"#
    let box = StartScript()
    let t = ScriptedTransport { request, _ in
        let a = request.arguments
        if a == ["_gui", "hello"] { return response(0, stdout: hello) }
        if a == ["_gui", "state"] { return response(0, stdout: state) }
        if a.prefix(2) == ["remote", "list"] { return response(0, stdout: list) }
        if a == ["remote", "template", "list", "--json"] { return response(0, stdout: templates) }
        if a.prefix(2) == ["remote", "launch"] {
            if let e = box.fail { return response(67, [.fail(FailEvent(code: .error, message: e))], stderr: e + "\n") }
            return response(0, stdout: #"{"version":1,"kind":"remote_launch","id":"\#(sid)","session":"# + (box.unlisted ? "null" : session) + "}")
        }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    let scheduler = Scheduler()
    scheduler.remoteInterval = 3600
    let m = makeModel(t, scheduler: scheduler)
    await m.start()
    func launches() -> [[String]] { t.requests.map(\.arguments).filter { $0.prefix(2) == ["remote", "launch"] } }
    func lists() -> Int { t.requests.filter { $0.arguments.prefix(2) == ["remote", "list"] }.count }
    m.setMainVisible(true)
    m.selection = .remoteSessions
    check("列表里有别的机器上允许这台 Mac 远程启动的（cca 有 remote_launch_machines）",
          await waitUntil { m.remote.doc != nil && !m.remoteLoading } && m.remoteLaunchMachinesAvailable
          && m.remotePeerLaunchers.map(\.machine) == ["office-linux", "home-mini"])
    guard let office = m.remotePeerLaunchers.first, let home = m.remotePeerLaunchers.last else {
        check("启动器", false)
        return
    }
    m.newRemoteSession(on: home)
    check("那台机器暂时没连着服务器：不开弹层", m.sheet == nil)
    m.newRemoteSession(on: office)
    guard case .remoteLaunch(let d)? = m.sheet else {
        check("新建会话：弹层", false)
        return
    }
    check("弹层：那台机器的模板；有两个时不先选，选了才能起；还没选模板时没有账号可挑", d.launcher == office && d.template == nil && !d.canStart
          && d.choices?.isEmpty == true && d.account == nil)
    d.choose(template: "4d4d4d4d")
    d.line = "顺便看看日志"
    check("选了 codex 的模板：那台机器上 codex 的账号只有一个，先选上；不能带开场白的模板：写了也不带",
          d.choices?.map(\.id) == ["a3a3a3a3a3a3a3a3"] && d.account == "a3a3a3a3a3a3a3a3" && d.sentAccount == "a3a3a3a3a3a3a3a3" && d.canStart && d.sentLine == nil)
    d.choose(template: "2b2b2b2b")
    check("换成 claude 的模板：codex 的账号不要了；claude 的账号有两个，不先选，选了才能起",
          d.choices?.map(\.id) == ["a1a1a1a1a1a1a1a1", "a2a2a2a2a2a2a2a2"] && d.account == nil && !d.canStart)
    d.account = "a3a3a3a3a3a3a3a3"
    check("选的不是这个模板能挑的：不能起", !d.canStart)
    d.account = "a2a2a2a2a2a2a2a2"
    d.choose(template: "2b2b2b2b")
    check("选了账号，再选一次同一个模板：账号留着", d.account == "a2a2a2a2a2a2a2a2" && d.canStart)
    d.account = "a1a1a1a1a1a1a1a1"
    d.line = "  先跑一遍测试 "
    check("能带开场白的：去掉两头的空白", d.sentLine == "先跑一遍测试" && d.sentAccount == "a1a1a1a1a1a1a1a1")
    box.fail = "没起成：模板在那台机器上改过了：刷新一下再起"
    let before = lists()
    m.startRemoteLaunch(d)
    let want = ["remote", "launch", office.peer, "2b2b2b2b", "--digest", "0a1b2c3d4e5f6a7b", "--size", "120x36", "--account", "a1a1a1a1a1a1a1a1",
                "--line", "先跑一遍测试", "--json"]
    check("起：带着列表里的摘要、终端的大小（还没开过终端：120x36）、挑的账号、开场白；起不成：cca 说的原因写在弹层里，列表再查一次，不开终端",
          await waitUntil { !d.starting && d.error != nil && lists() > before && !m.remoteLoading } && launches() == [want]
          && d.error == "没起成：模板在那台机器上改过了：刷新一下再起" && m.sheet == .remoteLaunch(d) && m.remoteTerminals.isEmpty,
          "\(launches()) \(d.error ?? "nil")")
    box.fail = nil
    m.startRemoteLaunch(d)
    check("起好了：弹层关上，在这一页里开一个终端接入新会话（能操作）",
          await waitUntil { m.sheet == nil && !d.starting && m.remoteTerminals.count == 1 } && d.error == nil
          && m.remoteTerminal?.session.id == sid && m.remoteTerminal?.readOnly == false && m.selection == .remoteSessions && launches().count == 2,
          "\(launches())")
    // 起好了，45 秒里服务器上一直没有它（cca 给的 session 是 null）：说一句，不接
    box.unlisted = true
    m.newRemoteSession(on: office)
    if case .remoteLaunch(let d2)? = m.sheet {
        d2.choose(template: "4d4d4d4d")
        m.startRemoteLaunch(d2)
        check("起好了但还没在服务器上登记好：弹层关上、说一句，不开终端",
              await waitUntil { m.sheet == nil && !d2.starting } && m.remoteTerminals.count == 1 && m.toast?.text.contains("还没在服务器上完成登记") == true
              && launches().last == ["remote", "launch", office.peer, "4d4d4d4d", "--digest", "1122334455667788", "--size", "120x36",
                                     "--account", "a3a3a3a3a3a3a3a3", "--json"],
              "\(launches().last ?? []) \(String(describing: m.toast))")
    } else {
        check("再开一次弹层", false)
    }
    // 正在起的时候关掉了弹层：照样起完；没起成就弹一条提示说原因（弹层没了）
    box.fail = "没起成：那台机器上开着的会话太多了：先结束几个"
    m.newRemoteSession(on: office)
    if case .remoteLaunch(let d3)? = m.sheet {
        d3.choose(template: "2b2b2b2b")
        d3.account = "a2a2a2a2a2a2a2a2"
        m.startRemoteLaunch(d3)
        m.dismissSheet()
        check("正在起的时候关掉了弹层：没起成弹一条提示，说 cca 给的原因",
              await waitUntil { !d3.starting } && m.sheet == nil && m.toast?.text == "没起成：那台机器上开着的会话太多了：先结束几个"
              && m.toast?.style == .warning && m.remoteTerminals.count == 1 && launches().count == 4,
              "\(String(describing: m.toast)) \(launches().count)")
    } else {
        check("再开一次弹层（中途关掉的）", false)
    }
    // 那台机器上的 cca 是旧版（peer_launchers 里没有 accounts）：模板用它自己的账号，不用挑、不带 --account
    let oldHost = try? DocumentDecoder.decode(RemoteDoc.self, from: Data(#"""
        {"version":1,"kind":"remote","sessions":[],"peers":[],"peer_launchers":[{"id":"6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6b","peer":"b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1",
         "machine":"office-linux","state":"ok","more":0,"accounts":null,"templates":[{"id":"2b2b2b2b","digest":"0a1b2c3d4e5f6a7b","name":"API：修 bug","tool":"claude",
         "account":"工作","host_account":"主力","dir":"~/srv/api","args":"","line":true}]}]}
        """#.utf8))
    if let l = oldHost?.peerLaunchers.first {
        let od = RemoteLaunchDraft(launcher: l, template: "2b2b2b2b")
        check("那台机器上的 cca 是旧版：不用挑账号（模板用它自己的），选了模板就能起，不带 --account",
              od.choices == nil && od.account == nil && od.sentAccount == nil && od.canStart)
    } else {
        check("旧版的启动器", false)
    }
    m.shutdown()
}
