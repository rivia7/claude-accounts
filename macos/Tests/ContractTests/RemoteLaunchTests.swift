import Foundation

// 远程启动（PROTOCOL 9.4）：模板的命令和文档、参数那一行字的拆和拼、App 里查模板、新建 / 修改 / 删掉模板、「允许远程启动」的开关。

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

private func words(_ s: String) -> [String]? {
    if case .success(let w) = ShellWords.split(s) { return w }
    return nil
}

private func wordsProblem(_ s: String) -> ShellWords.Problem? {
    if case .failure(let p) = ShellWords.split(s) { return p }
    return nil
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
    let add = CcaCommand.remoteTemplateSave(name: "网站", tool: .claude, account: "工作(bob@example.com)", dir: "/Users/me/projects/site",
                                            args: ["--model", "opus"], replace: nil)
    check("存模板：都给全（cca 不在终端里问），模板的参数放在 -- 后面",
          add.arguments == ["remote", "template", "add", "--name", "网站", "--tool", "claude", "--account", "工作(bob@example.com)",
                            "--dir", "/Users/me/projects/site", "--", "--model", "opus"], "\(add.arguments)")
    let replace = CcaCommand.remoteTemplateSave(name: "队列", tool: .codex, account: "frank@example.com", dir: "/Users/me/srv/jobs", args: [],
                                                replace: "5e6f7a8b")
    check("改模板：带 --replace <编号>；没有参数也有 --",
          replace.arguments == ["remote", "template", "add", "--name", "队列", "--tool", "codex", "--account", "frank@example.com",
                                "--dir", "/Users/me/srv/jobs", "--replace", "5e6f7a8b", "--"], "\(replace.arguments)")
    check("参数里的 --、以 - 开头的原样放在 -- 后面",
          CcaCommand.remoteTemplateSave(name: "n", tool: .claude, account: "a", dir: "/d", args: ["--", "-x"], replace: nil).arguments.suffix(3) == ["--", "--", "-x"])
    check("删模板：按编号", CcaCommand.remoteTemplateRemove(id: "1a2b3c4d").arguments == ["remote", "template", "rm", "1a2b3c4d"])
    check("允许、不允许远程启动：按配对的 id", CcaCommand.remoteLaunch(peer: pid, on: true).arguments == ["remote", "allow", pid, "launch", "on"]
          && CcaCommand.remoteLaunch(peer: pid, on: false).arguments == ["remote", "allow", pid, "launch", "off"])
    let changes = [add, replace, CcaCommand.remoteTemplateRemove(id: "1a2b3c4d"), CcaCommand.remoteLaunch(peer: pid, on: true)]
    check("改模板、改远程启动和改配对表的几条一个接一个；都不是哪个账号的",
          changes.allSatisfy { $0.gate != nil && $0.gate == CcaCommand.remoteUnpair(peer: pid).gate && !$0.runsToCompletion && $0.tool == nil && $0.key == nil })
    check("同意项认识", ConsentItem.remoteLaunch.isKnown && ConsentItem.remoteLaunch.rawValue == "remote_launch")
    check("hello 的 remote_launch；旧版 cca 没有",
          (try? DocumentDecoder.decode(HelloDoc.self, from: Data(#"{"version":1,"kind":"hello","api":1,"remote":1,"remote_launch":1}"#.utf8)))?.remoteLaunch == 1
          && (try? DocumentDecoder.decode(HelloDoc.self, from: Data(#"{"version":1,"kind":"hello","api":1,"remote":1}"#.utf8)))?.remoteLaunch == nil)

    // 参数那一行字：照 shell 的写法拆开、拼回去
    check("拆参数：空白隔开（几个空格、Tab、换行都一样）", words("  --model   opus\t--verbose\n-c ") == ["--model", "opus", "--verbose", "-c"])
    check("什么都没写是没有参数", words("") == [] && words(" \t\n ") == [])
    check("单引号里的原样（反斜杠、双引号、$ 都不特别）",
          words(#"--append-system-prompt '说中文，"别" \改 $HOME'"#) == ["--append-system-prompt", #"说中文，"别" \改 $HOME"#])
    check("双引号里：反斜杠只让双引号、反斜杠、$、反引号原样，别的反斜杠留着", words(#""a \"b\" \\ \$x \` \n""#) == [#"a "b" \ $x ` \n"#],
          "\(String(describing: words(#""a \"b\" \\ \$x \` \n""#)))")
    check("引号外的反斜杠让下一个字符原样（空格、引号）", words(#"a\ b \'c \"d"#) == ["a b", "'c", "\"d"])
    check("引号和字连着是一项；空的引号也是一项", words(#"--name='a b'c "" ''"#) == ["--name=a bc", "", ""])
    check("反斜杠加换行是续行", words("--model \\\nopus") == ["--model", "opus"])
    check("~、$HOME、* 不展开：原样交给 cca", words("~/x $HOME *.md") == ["~/x", "$HOME", "*.md"])
    check("引号没配对、最后多一个反斜杠：说哪里不对，不猜", wordsProblem("--x 'abc") == .openSingle && wordsProblem(#"--x "abc"#) == .openDouble
          && wordsProblem(#""a\""#) == .openDouble && wordsProblem(#"abc\"#) == .trailingBackslash
          && ShellWords.Problem.openSingle.message == "参数里的单引号没有配对")
    let samples: [[String]] = [[], ["--model", "opus"], ["--append-system-prompt", "说中文，改完跑一遍测试"],
                               ["a b", "it's", #""q""#, "$HOME", "~", "", #"back\slash"#, "tab\tx", "new\nline", "*", "--", "-", "a=b,c:d@e%f+g"]]
    check("拼成一行再拆开：原样", samples.allSatisfy { words(ShellWords.join($0)) == $0 }, samples.map(ShellWords.join).joined(separator: " | "))
    check("拼的时候只引要引的", ShellWords.join(["--model", "opus", "a b", "it's", ""]) == #"--model opus 'a b' 'it'\''s' ''"#,
          ShellWords.join(["--model", "opus", "a b", "it's", ""]))

    // 模板的文档
    guard let d = fixture("remote_templates.json"), let doc = try? DocumentDecoder.decode(RemoteTemplatesDoc.self, from: d) else {
        check("夹具 remote_templates.json", false)
        return
    }
    let t = doc.templates
    check("模板：编号、名字、工具、账号、文件夹、参数", t.map(\.id) == ["1a2b3c4d", "5e6f7a8b", "9c0d1e2f"] && t.map(\.tool) == [.claude, .codex, .claude]
          && t[0].name == "网站：修 bug" && t[0].account == "bob@example.com" && t[0].accountName == "工作" && t[0].dir == "/Users/me/projects/site"
          && t[0].args == ["--model", "opus", "--append-system-prompt", "说中文，改完跑一遍测试"] && t[1].args.isEmpty && t.allSatisfy(\.line))
    check("用不了的说原因：账号没了（现在叫什么是 null）", t[2].problem == .noAccount && t[2].accountName == nil && t.prefix(2).allSatisfy { $0.problem == nil })
    check("启动器：在跑、几个模板、几台手机", doc.launcher == RemoteLauncher(running: true, templates: 3, peers: 1))
    check("按编号找", doc.template("5e6f7a8b")?.name == "任务队列" && doc.template("nope") == nil)
    let odd = try? DocumentDecoder.decode(RemoteTemplatesDoc.self, from: Data(#"""
        {"version":1,"kind":"remote_templates","templates":[{"id":"aa","name":"x","tool":"zz_new","account":"a@example.com","dir":"/x",
         "args":["--a",1,"b"],"problem":"zz_new"},{"name":"没有编号"}]}
        """#.utf8))
    check("没有编号的丢掉；不认识的工具、原因留着；参数里不是字符串的丢掉；没有 launcher 是 nil",
          odd?.templates.count == 1 && odd?.templates.first?.tool?.isKnown == false && odd?.templates.first?.problem?.isKnown == false
          && odd?.templates.first?.args == ["--a", "b"] && odd?.templates.first?.line == false && odd?.launcher == nil)

    // 列表里的：哪台手机能远程启动、启动器
    guard let ld = fixture("remote_list.json"), let rl = try? DocumentDecoder.decode(RemoteDoc.self, from: ld) else {
        check("夹具 remote_list.json", false)
        return
    }
    check("列表：哪台手机能远程启动、启动器", rl.peers.map(\.launch) == [false, false, true, false] && rl.launcher == RemoteLauncher(running: true, templates: 3, peers: 1))
    let old = try? DocumentDecoder.decode(RemoteDoc.self, from: Data(#"{"version":1,"kind":"remote","sessions":[],"peers":[{"id":"aa","web":true,"incoming":"control"}]}"#.utf8))
    check("旧版 cca 没有这两项：不算能远程启动，没有启动器", old?.peers.first?.launch == false && old?.launcher == nil)

    // 文字
    check("模板的第二行：账号（现在叫什么，没了是存着的邮箱）、文件夹、参数（照 shell 的写法）",
          Texts.remoteTemplateDetail(t[0], dir: "~/projects/site") == "工作 · ~/projects/site · --model opus --append-system-prompt '说中文，改完跑一遍测试'"
          && Texts.remoteTemplateDetail(t[1], dir: "~/srv/jobs") == "frank@example.com · ~/srv/jobs"
          && Texts.remoteTemplateDetail(t[2], dir: "~/notes") == "old@example.com · ~/notes · --permission-mode plan", Texts.remoteTemplateDetail(t[0], dir: "~/projects/site"))
    check("用不了的原因：短的标在名字后面，不认识的也有一句", Texts.remoteTemplateProblem(.noAccount) == "账号没了" && Texts.remoteTemplateProblem(.noRelay) == "不经中转"
          && Texts.remoteTemplateProblem(RemoteTemplateProblem(rawValue: "zz_new")) == "现在用不了"
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
    check("开关：开着的、能开的不拦；不是手机的、只让看的说原因（cca 也这么查）",
          phone.launch && m.remoteLaunchBlocked(phone) == nil && m.remoteLaunchBlocked(spare) == nil
          && m.remoteLaunchBlocked(mini) == "只有手机能远程启动会话" && m.remoteLaunchBlocked(viewer)?.contains("看和操作") == true)
    var n = asked().count
    m.setRemoteLaunch(phone, true)
    m.setRemoteLaunch(viewer, true)
    m.setRemoteLaunch(mini, true)
    check("没变的、拦着的、不是手机的：不调命令", asked().count == n && !m.remoteBusy)
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
    check("新建：选上现在的工具、它上次用的账号；名字、文件夹空着不能存",
          draft.original == nil && draft.tool == .claude && draft.account == "工作(bob@example.com)" && draft.name.isEmpty && draft.args.isEmpty
          && !draft.canSave && m.remoteTemplateTools == [.claude, .codex])
    m.setRemoteTemplateTool(draft, .codex)
    check("换了工具：账号换成这个工具上次用的", draft.tool == .codex && draft.account == "frank@example.com")
    m.setRemoteTemplateTool(draft, .claude)
    draft.name = "  网站：写文档  "
    m.hooks.chooseTemplateDirectory = { _ in "/Users/me/projects/docs" }
    m.chooseRemoteTemplateDirectory(draft)
    check("选文件夹", await waitUntil { draft.dir == "/Users/me/projects/docs" } && draft.canSave && draft.account == "工作(bob@example.com)")
    draft.args = "--model opus --append-system-prompt '说中文"
    n = asked().count
    m.saveRemoteTemplate(draft)
    check("参数拆不开：弹层里说，不调 cca", draft.error == "参数里的单引号没有配对" && asked().count == n && m.sheet == .remoteTemplate(draft) && !draft.saving)
    draft.args = "--model opus --append-system-prompt '说中文，别改测试'"
    box.saveError = "已经有叫「网站：写文档」的模板了"
    m.saveRemoteTemplate(draft)
    let added = ["remote", "template", "add", "--name", "网站：写文档", "--tool", "claude", "--account", "工作(bob@example.com)",
                 "--dir", "/Users/me/projects/docs", "--", "--model", "opus", "--append-system-prompt", "说中文，别改测试"]
    check("存：名字去掉两头的空白，参数拆开放在 -- 后面；cca 说不行：原因写在弹层里，接着改",
          await waitUntil { !draft.saving && draft.error != nil } && asked().last == added && draft.error == "已经有叫「网站：写文档」的模板了"
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
    check("修改：填好原来的；账号按存着的邮箱认出账号表里的那个；参数拼回一行（照 shell 的写法）",
          e.original?.id == "1a2b3c4d" && e.name == "网站：修 bug" && e.tool == .claude && e.account == "工作(bob@example.com)"
          && e.dir == "/Users/me/projects/site" && e.args == "--model opus --append-system-prompt '说中文，改完跑一遍测试'" && e.canSave, e.args)
    e.args += " --verbose"
    m.saveRemoteTemplate(e)
    let replaced = ["remote", "template", "add", "--name", "网站：修 bug", "--tool", "claude", "--account", "工作(bob@example.com)",
                    "--dir", "/Users/me/projects/site", "--replace", "1a2b3c4d", "--", "--model", "opus", "--append-system-prompt",
                    "说中文，改完跑一遍测试", "--verbose"]
    check("存修改：带 --replace <编号>", await waitUntil { m.sheet == nil && !e.saving && settled() } && asked().contains(replaced), "\(asked().suffix(3))")
    m.editRemoteTemplate(gone)
    if case .remoteTemplate(let g)? = m.sheet {
        check("修改账号没了的模板：账号空着，要重新选才能存", g.account.isEmpty && g.tool == .claude && g.original?.id == "9c0d1e2f" && !g.canSave)
    } else {
        check("修改账号没了的模板：弹层", false)
    }
    m.dismissSheet()
    let agyDoc = try? DocumentDecoder.decode(RemoteTemplatesDoc.self, from: Data(
        #"{"version":1,"kind":"remote_templates","templates":[{"id":"0a0b0c0d","name":"agy","tool":"agy","account":"carol@example.com","dir":"/x","args":[]}]}"#.utf8))
    if let agy = agyDoc?.templates.first {
        m.editRemoteTemplate(agy)
        if case .remoteTemplate(let g)? = m.sheet {
            check("模板的工具没启用：换成能选的，账号重新选", g.tool == .claude && g.account.isEmpty)
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
