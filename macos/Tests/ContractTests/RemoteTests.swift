import Foundation

// 远程会话（PROTOCOL 9）：命令的样子、列表的解码、只显示的页面（配对码）、「远程会话」一页什么时候查、
// 界面里的终端（起进程、字节进出、大小、断开、结束）、真的伪终端。

/// 假的终端句柄：记下写了什么、改过的大小、断开了几次；测试自己喂输出、报退出。
final class FakeTerminal: TerminalHandle, @unchecked Sendable {
    let arguments: [String]
    let cols: Int
    let rows: Int
    private let onEvent: @Sendable (PtyEvent) -> Void
    private let lock = NSLock()
    private var _written = Data()
    private var _sizes: [[Int]] = []
    private var _closed = 0

    init(arguments: [String], cols: Int, rows: Int, onEvent: @escaping @Sendable (PtyEvent) -> Void) {
        self.arguments = arguments
        self.cols = cols
        self.rows = rows
        self.onEvent = onEvent
    }

    var written: Data { lock.withLock { _written } }
    var sizes: [[Int]] { lock.withLock { _sizes } }
    var closed: Int { lock.withLock { _closed } }

    func write(_ data: Data) { lock.withLock { _written.append(data) } }
    func resize(cols: Int, rows: Int) { lock.withLock { _sizes.append([cols, rows]) } }
    func close() { lock.withLock { _closed += 1 } }
    func emit(_ e: PtyEvent) { onEvent(e) }
}

/// 伪终端上读到的东西（线程安全）。
private final class PtyLog: @unchecked Sendable {
    private let lock = NSLock()
    private var _text = ""
    private var _exit: Int32?
    var text: String { lock.withLock { _text } }
    var exit: Int32? { lock.withLock { _exit } }
    func add(_ e: PtyEvent) {
        lock.withLock {
            switch e {
            case .output(let d): _text += String(decoding: d, as: UTF8.self)
            case .exited(let c): _exit = c
            }
        }
    }
}

private final class RemoteSwitch: @unchecked Sendable {
    private let lock = NSLock()
    private var _failing = false
    var failing: Bool {
        get { lock.withLock { _failing } }
        set { lock.withLock { _failing = newValue } }
    }
}

func remoteProtocolTests() {
    section("远程会话：命令和文档")
    let auto = CcaCommand.remoteList(local: false, auto: true)
    check("命令：cca remote list --json；定时刷新低优先级，点「刷新」的不降", auto.arguments == ["remote", "list", "--json"] && auto.isBackground
          && !CcaCommand.remoteList(local: false, auto: false).isBackground && !auto.runsToCompletion && auto.tool == nil && auto.key == nil)
    check("只要这台 Mac 上的", CcaCommand.remoteList(local: true, auto: false).arguments == ["remote", "list", "--local", "--json"])
    check("开关", CcaCommand.remoteSwitch(on: true).arguments == ["remote", "on"] && CcaCommand.remoteSwitch(on: false).arguments == ["remote", "off"])
    let pid = String(repeating: "b1", count: 16)
    check("改对方能做什么、取消配对：按配对的 id", CcaCommand.remoteAllow(peer: pid, access: .view).arguments == ["remote", "allow", pid, "view"]
          && CcaCommand.remoteUnpair(peer: pid).arguments == ["remote", "unpair", pid])
    check("改配对表的几条一个接一个", CcaCommand.remoteAllow(peer: pid, access: .none).gate == CcaCommand.remoteUnpair(peer: pid).gate
          && CcaCommand.remoteSwitch(on: true).gate != nil)
    check("配对是提问会话（App 里问完）", Handoff.remotePair.arguments == ["remote", "pair"] && Handoff.remoteJoin.arguments == ["remote", "join"]
          && Handoff.remotePair.inApp && Handoff.remoteJoin.inApp && !Handoff.remotePair.needsForms)
    check("结果码认识", ResultCode.remotePaired.isKnown && ResultCode.remoteUpdated.isKnown && ConsentItem.remotePair.isKnown)
    let sid = String(repeating: "3a", count: 16)
    check("结束会话：按会话的 id，和改配对表的几条一个接一个；同意项认识", CcaCommand.remoteEnd(session: sid).arguments == ["remote", "end", sid]
          && CcaCommand.remoteEnd(session: sid).gate == CcaCommand.remoteUnpair(peer: pid).gate && !CcaCommand.remoteEnd(session: sid).runsToCompletion
          && CcaCommand.remoteEnd(session: sid).tool == nil && ConsentItem.remoteEnd.isKnown && ConsentItem.remoteEnd.rawValue == "remote_end")
    check("hello 的 remote_end；旧版 cca 没有", (try? DocumentDecoder.decode(HelloDoc.self, from: Data(#"{"version":1,"kind":"hello","api":1,"remote":1,"remote_end":1}"#.utf8)))?.remoteEnd == 1
          && (try? DocumentDecoder.decode(HelloDoc.self, from: Data(#"{"version":1,"kind":"hello","api":1,"remote":1}"#.utf8)))?.remoteEnd == nil)
    check("hello 的 remote", (try? DocumentDecoder.decode(HelloDoc.self, from: Data(#"{"version":1,"kind":"hello","api":1,"remote":1}"#.utf8)))?.remote == 1)
    check("旧版 cca 的 hello 没有 remote", (try? DocumentDecoder.decode(HelloDoc.self, from: Data(tinyHello.utf8)))?.remote == nil)

    guard let d = fixture("remote_list.json"), let doc = try? DocumentDecoder.decode(RemoteDoc.self, from: d) else {
        check("夹具 remote_list.json", false)
        return
    }
    check("列表：开关、中转、会话、配对过的机器", doc.enabled && doc.relayAvailable && doc.sessions.count == 4 && doc.peers.count == 3 && doc.errors.isEmpty)
    let local = doc.sessions.first { $0.isLocal }, far = doc.sessions.filter { !$0.isLocal }
    check("这台 Mac 上的会话：知道启动它的终端还开着", local?.localTerminal == true && local?.peer == nil && local?.access == .control && local?.attachable == true)
    check("别的机器上的：哪个配对的、机器名、只让看的、不在线的",
          far.count == 3 && far.allSatisfy { $0.peer != nil && $0.machine != nil && $0.localTerminal == nil }
          && far.contains { $0.access == .view && $0.tool == .codex } && far.contains { $0.state == .offline && !$0.attachable })
    check("会话的 peer 都是配对过的机器", far.allSatisfy { s in doc.peers.contains { $0.id == s.peer } })
    check("配对过的机器：它能做什么、谁显示的配对码、对方取消了的", doc.peers.map(\.incoming) == [.none, .control, .view]
          && doc.peers.map(\.shown) == [false, true, true] && doc.peers.last?.state == .gone)
    check("按 id 找会话", doc.session(far[0].id)?.machine == "office-linux" && doc.session("nope") == nil)

    let problem = #"{"version":1,"kind":"remote","generated_epoch":1,"enabled":false,"relay":{"available":false,"message":"没有配置维护者的映射"},"sessions":[{"id":"aa","where":"zz_new","state":"zz","access":"zz"},{"short":"没有 id"}],"peers":[{"id":"bb","incoming":"zz"}],"errors":[{"code":"relay_off","message":"没有配置维护者的映射"},{"code":"zz_new","message":"别的"}]}"#
    let pd = try? DocumentDecoder.decode(RemoteDoc.self, from: Data(problem.utf8))
    check("用不了中转：原因照给；没有 id 的行丢掉，不认识的值留着", pd?.relayAvailable == false && pd?.relayMessage == "没有配置维护者的映射"
          && pd?.sessions.count == 1 && pd?.sessions.first?.short == "aa" && pd?.sessions.first?.attachable == false
          && pd?.peers.first?.incoming.isKnown == false && pd?.errors.map(\.code.isKnown) == [true, false])

    // 文字
    let s = far.first { $0.viewers == 1 }!
    check("会话的名字和第二行", Texts.remoteTitle(s) == "Claude Code · 工作"
          && Texts.remoteDetail(s, now: (s.sinceEpoch ?? 0) + 7980) == "~/srv/api · 开了 2 小时 13 分 · 另有 1 处接着", Texts.remoteDetail(s, now: (s.sinceEpoch ?? 0) + 7980))
    check("这台 Mac 上的会话说启动它的终端还在不在", Texts.remoteDetail(local!, now: (local!.sinceEpoch ?? 0) + 60).hasSuffix("启动它的终端还开着"))
    check("接不上的说原因", Texts.remoteStateNote(far.first { $0.state == .offline }!) == "那台机器不在线" && Texts.remoteStateNote(s) == nil)
    check("终端结束后的那句话：这边断开的、会话结束的、没接上的",
          Texts.remoteEnded(code: 0, closedByUser: true, listed: true) == "已断开，会话在那边照常运行"
          && Texts.remoteEnded(code: 0, closedByUser: false, listed: false) == "会话已经结束"
          && Texts.remoteEnded(code: 3, closedByUser: false, listed: false) == "会话已经结束（退出码 3）"
          && Texts.remoteEnded(code: 0, closedByUser: false, listed: true) == "已断开，会话在那边照常运行"
          && Texts.remoteEnded(code: 66, closedByUser: false, listed: true) == "没能经服务器接上那台机器"
          && Texts.remoteEnded(code: 65, closedByUser: false, listed: false).hasPrefix("没有这个会话了")
          && Texts.remoteEnded(code: 68, closedByUser: false, listed: true).hasPrefix("连接断了"))

    check("终端的标题（对方写的）显示之前过滤：控制字符、改文字方向的、零宽的去掉，换行当空格，截短",
          Texts.plainLine("a\u{1b}[31mb\r\nc\u{202e}d\u{200b}e\u{2028}f\u{e0041}g\u{feff}", max: 50) == "a[31mbcde fg"
          && Texts.plainLine("  ✳ 写测试 — 工作  ", max: 50) == "✳ 写测试 — 工作" && Texts.plainLine(String(repeating: "长", count: 300), max: 120).count == 120)
    let bare = try? DocumentDecoder.decode(RemoteDoc.self, from: Data(#"{"version":1,"kind":"remote","sessions":[{"id":"aa"}],"peers":[{"id":"bb"}]}"#.utf8))
    check("缺的字段往保守的方向取：不算接得上、只看；配对的不让接入、状态不知道", bare?.sessions.first?.attachable == false && bare?.sessions.first?.access == .view
          && bare?.peers.first?.incoming == RemoteAccess.none && bare?.peers.first?.state == .unknown && bare?.enabled == false && bare?.relayAvailable == false)

    // 只显示的页面（配对码）：没有要回答的
    let page = #"{"v":1,"ev":"ask","seq":2,"q":"remote.pair","title":"和另一台机器配对","message":"M","fields":[{"id":"code","kind":"code","label":"配对码","value":"7K2M-9QXD-4HTP-W3NB","hint":"正在等"}]}"#
    let ask = EventLog.parseLine(Data(page.utf8))?.ask
    check("配对码那一页：能显示、没有要回答的", ask?.isDisplayable == true && ask?.displayOnly == true && ask?.fields.first?.value == "7K2M-9QXD-4HTP-W3NB")
    let mixed = AskEvent(seq: 1, q: "x", title: "", message: "", fields: [PromptField(id: "c", kind: .code), PromptField(id: "next", kind: .choice,
                                                                                                                      options: [PromptOption(value: "1", label: "a")])])
    check("带着选择题的代码页还是要回答的", !mixed.displayOnly && !AskEvent(seq: 1, q: "x", title: "", message: "", fields: []).displayOnly)
}

@MainActor func remoteFlowTests() async {
    section("远程会话：配对码那一页")
    let s = ScriptedSession()
    var ends: [FlowModel.End] = []
    let f = FlowModel(handoff: .remotePair, session: s) { _, e in ends.append(e) }
    s.feed(.ask(AskEvent(seq: 1, q: "consent.remote_pair", title: "和另一台机器配对", message: "",
                         fields: [PromptField(id: "v", kind: .confirm, label: "显示配对码吗？", defaultValue: "y")])))
    _ = await waitUntil { if case .asking = f.phase { return true }; return false }
    f.confirm(true)
    check("先问要不要显示配对码", s.answers == [["1 v y"]])
    s.feed(.ask(AskEvent(seq: 2, q: "remote.pair", title: "和另一台机器配对", message: "在另一台机器上输入",
                         fields: [PromptField(id: "code", kind: .code, label: "配对码", value: "7K2M-9QXD-4HTP-W3NB")])))
    _ = await waitUntil { if case .asking(let e) = f.phase { return e.seq == 2 }; return false }
    check("配对码那一页不能提交", !f.canSubmit)
    f.submit()
    check("点了也不交答案（cca 对不上字段会停下），页面还在", s.answers.count == 1 && { if case .asking(let e) = f.phase { return e.seq == 2 }; return false }())
    s.feed(.result(ResultEvent(code: .remotePaired, route: nil, node: nil, port: nil, message: "已经和「home-mini」配对", tool: .claude, key: nil)))
    s.finish(0)
    _ = await waitUntil { !ends.isEmpty }
    check("对方输入了：cca 自己收场，带结果", ends.first == .success(ResultEvent(code: .remotePaired, route: nil, node: nil, port: nil,
                                                                              message: "已经和「home-mini」配对", tool: .claude, key: nil)))

    let s2 = ScriptedSession()
    var ends2: [FlowModel.End] = []
    let f2 = FlowModel(handoff: .remotePair, session: s2) { _, e in ends2.append(e) }
    s2.feed(.ask(AskEvent(seq: 1, q: "remote.pair", title: "", message: "", fields: [PromptField(id: "code", kind: .code, value: "X")])))
    _ = await waitUntil { if case .asking = f2.phase { return true }; return false }
    f2.cancel()
    check("不想配了：取消关掉答案管道（cca 把后台的配对停掉）", s2.closed == 1 && ends2 == [.cancelled])
}

@MainActor func remoteModelTests() async {
    section("远程会话：页面和终端")
    guard let helloData = fixture("hello.json"), let state = fixture("state.json").map({ String(decoding: $0, as: UTF8.self) }),
          let list = fixture("remote_list.json").map({ String(decoding: $0, as: UTF8.self) }) else {
        check("夹具", false)
        return
    }
    let hello = String(decoding: helloData, as: UTF8.self)

    // 旧版 cca：没有这一页
    do {
        let old = hello.replacingOccurrences(of: #""remote": 1,"#, with: "")
        let t = ScriptedTransport { request, _ in
            if request.arguments == ["_gui", "hello"] { return response(0, stdout: old) }
            if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
            return response(0, stdout: SampleDocs.clientUpdates)
        }
        let m = makeModel(t)
        await m.start()
        m.setMainVisible(true)
        m.selection = .remoteSessions
        m.refreshRemote(manual: true)
        m.setRemoteEnabled(true)
        m.pairRemote(show: true)
        try? await Task.sleep(nanoseconds: 100_000_000)
        check("旧版 cca（hello 没有 remote）：没有这一页，不查、不调命令", !m.remoteAvailable && m.selection == .office && !m.scheduler.remotePollingRunning
              && !t.requests.contains { $0.arguments.first == "remote" } && t.started.isEmpty && m.flow == nil)
        m.shutdown()
    }

    // 有这一页、还不能结束会话的 cca（3.5.2-beta.2）：不给「结束」，也不调那条命令
    do {
        let old = hello.replacingOccurrences(of: #""remote_end": 1,"#, with: "")
        let t = ScriptedTransport { request, _ in
            if request.arguments == ["_gui", "hello"] { return response(0, stdout: old) }
            if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
            if request.arguments.prefix(2) == ["remote", "list"] { return response(0, stdout: list) }
            return response(0, stdout: SampleDocs.clientUpdates)
        }
        let m = makeModel(t)
        await m.start()
        m.refreshRemote(manual: true)
        let got = await waitUntil { m.remote.doc != nil }
        if let near = m.remote.doc?.sessions.first(where: { $0.isLocal }) { m.endRemote(near) }
        try? await Task.sleep(nanoseconds: 100_000_000)
        check("hello 没有 remote_end：有这一页，不能结束会话", got && m.remoteAvailable && !m.remoteEndAvailable
              && !t.requests.contains { $0.arguments.prefix(2) == ["remote", "end"] })
        m.shutdown()
    }

    let sw = RemoteSwitch()
    let t = ScriptedTransport { request, _ in
        if request.arguments == ["_gui", "hello"] { return response(0, stdout: hello) }
        if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
        if request.arguments.prefix(2) == ["remote", "list"] {
            if sw.failing { return response(1, stderr: "读不了会话目录\n") }
            return response(0, stdout: list)
        }
        if request.arguments.first == "remote" {
            return response(0, [.result(ResultEvent(code: .remoteUpdated, route: nil, node: nil, port: nil, message: "「office-linux」只能看这台机器上的会话"))])
        }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    let log = HookLog()
    let scheduler = Scheduler()
    scheduler.remoteInterval = 3600
    let m = makeModel(t, log: log, scheduler: scheduler)
    await m.start()
    func asked() -> [[String]] { t.requests.map(\.arguments).filter { $0.first == "remote" } }
    check("cca 有远程会话", m.remoteAvailable)
    check("看不见的时候不查", !m.scheduler.remotePollingRunning && asked().isEmpty)
    m.setMainVisible(true)
    m.selection = .remoteSessions
    check("打开这一页：马上查一次，之后定时查", m.scheduler.remotePollingRunning)
    check("查到了", await waitUntil { m.remote.doc?.sessions.count == 4 }, "\(asked())")
    check("问的是 cca remote list --json", asked().first == ["remote", "list", "--json"] && m.remote.error == nil && m.remote.fetchedEpoch == 1794042000)

    sw.failing = true
    m.refreshRemote(manual: true)
    check("查不成：上次查到的照样显示，旁边说原因", await waitUntil { m.remote.error != nil && !m.remoteLoading } && m.remote.doc?.sessions.count == 4,
          m.remote.error ?? "nil")
    sw.failing = false
    m.refreshRemote(manual: true)
    check("又查成了：原因去掉", await waitUntil { m.remote.error == nil && !m.remoteLoading })

    // 改对方能做什么：做完说一句、再查一次
    let peer = m.remote.doc!.peers[1]
    let n = asked().count
    m.setRemoteAccess(peer, peer.incoming)
    check("没变的不调命令", asked().count == n && !m.remoteBusy)
    m.setRemoteAccess(peer, .view)
    check("改成只看：cca remote allow <配对的 id> view，做完再查一次",
          await waitUntil { asked().count >= n + 2 && !m.remoteBusy } && asked()[n] == ["remote", "allow", peer.id, "view"]
          && asked().last == ["remote", "list", "--json"], "\(asked().suffix(3))")
    check("显示 cca 说的那句话", m.toast?.text.contains("只能看这台机器上的会话") == true, "\(String(describing: m.toast))")
    m.unpairRemote(peer)
    check("取消配对", await waitUntil { asked().contains(["remote", "unpair", peer.id]) && !m.remoteBusy })
    m.setRemoteEnabled(false)
    check("关掉开关", await waitUntil { asked().contains(["remote", "off"]) && !m.remoteBusy })

    // 结束会话：只管这台 Mac 上的
    if let near = m.remote.doc?.sessions.first(where: { $0.isLocal }), let away = m.remote.doc?.sessions.first(where: { !$0.isLocal }) {
        check("cca 能结束会话（hello.remote_end）", m.remoteEndAvailable)
        let k = asked().count
        m.endRemote(away)
        check("别的机器上的会话不归这里结束", asked().count == k && !m.remoteBusy)
        m.endRemote(near)
        check("结束这台 Mac 上的会话：cca remote end <会话的 id>，做完再查一次",
              await waitUntil { asked().count >= k + 2 && !m.remoteBusy } && asked()[k] == ["remote", "end", near.id]
              && asked().last == ["remote", "list", "--json"], "\(asked().suffix(3))")
    } else {
        check("夹具里有这台 Mac 上的和别的机器上的会话", false)
    }

    // 配对：提问会话
    let session = ScriptedSession()
    t.sessions = [session]
    m.pairRemote(show: true)
    check("显示配对码：在 App 里问（cca remote pair）", await waitUntil { m.flow != nil } && t.started.last == ["remote", "pair"])
    let before = asked().count
    session.feed(.result(ResultEvent(code: .remotePaired, route: nil, node: nil, port: nil, message: "已经和「home-mini」配对", tool: .claude, key: nil)))
    session.finish(0)
    check("配好了：马上再查一次（列出那台机器和它上面的会话）", await waitUntil { m.flow == nil && asked().count > before })

    // 界面里的终端
    _ = await waitUntil { !m.remoteLoading }
    let far = m.remote.doc!.sessions.first { !$0.isLocal && $0.access == .control && $0.attachable }!
    m.attachRemote(far, readOnly: false)
    guard let term = m.remoteTerminal else {
        check("接入：这一页换成终端", false)
        return
    }
    check("接入：这一页换成终端；还没量好大小，进程还没起", term.status == .waiting && t.terminals.isEmpty && m.selection == .remoteSessions && log.shown > 0)
    term.resize(cols: 132, rows: 40)
    guard let h = t.terminals.first else {
        check("界面量好大小：起 cca remote attach", false)
        return
    }
    check("界面量好大小：起 cca remote attach <会话的 id>，带着列数行数", h.arguments == ["remote", "attach", far.id] && h.cols == 132 && h.rows == 40
          && term.status == .running)
    h.emit(.output(Data("abc".utf8)))
    h.emit(.output(Data("def".utf8)))
    var drawn = ""
    term.sink = { drawn += String(decoding: $0, as: UTF8.self) }
    check("视图接上来之前的输出先攒着，接上来时按顺序交给它", drawn == "abcdef")
    h.emit(.output(Data("ghi".utf8)))
    check("之后的直接给视图", drawn == "abcdefghi")
    term.write(Data("ls\r".utf8))
    term.resize(cols: 100, rows: 30)
    check("按键原样写给伪终端；大小变了只改大小，不再起进程", h.written == Data("ls\r".utf8) && h.sizes == [[100, 30]] && t.terminals.count == 1)
    m.selection = .office
    check("换到别的页：终端还接着", term.status == .running && h.closed == 0 && m.remoteTerminal === term)
    m.mainWindowClosed()
    check("主窗口关了：断开（发信号，等进程自己退出）", h.closed == 1 && term.closedByUser && term.status == .running)
    let listed = asked().count
    h.emit(.exited(0))
    let again = await waitUntil { asked().count > listed }
    check("进程退出：终端留着（看得到最后的输出），再查一次列表", term.status == .ended(0) && m.remoteTerminal === term && again)
    _ = await waitUntil { !m.remoteLoading }
    let back = asked().count
    m.closeRemoteTerminal()
    let once = await waitUntil { asked().count == back + 1 && !m.remoteLoading }
    check("返回列表：早就结束了的，现在查一次", m.remoteTerminal == nil && once)

    // 只让看的会话：带 -r；会话自己结束
    let viewOnly = m.remote.doc!.sessions.first { $0.access == .view }!
    m.attachRemote(viewOnly, readOnly: false)
    m.remoteTerminal?.resize(cols: 80, rows: 24)
    check("那台机器只让看：带 -r", m.remoteTerminal?.readOnly == true && t.terminals.last?.arguments == ["remote", "attach", "-r", viewOnly.id])
    t.terminals.last?.emit(.exited(3))
    check("会话自己结束：记下退出码，不算这边断开的", m.remoteTerminal?.status == .ended(3) && m.remoteTerminal?.closedByUser == false)
    // 已经开着一个时再接入另一个：先断开原来的
    m.attachRemote(far, readOnly: true)
    m.remoteTerminal?.resize(cols: 80, rows: 24)
    let first = t.terminals.last!
    _ = await waitUntil { !m.remoteLoading }
    let swapped = asked().count
    m.attachRemote(viewOnly, readOnly: true)
    check("一次接一个：再接入别的，原来的先断开", first.closed == 1 && m.remoteTerminal?.session.id == viewOnly.id && first.arguments == ["remote", "attach", "-r", far.id])
    check("断开的那一刻不查列表：等它的进程真的退出", asked().count == swapped)
    first.emit(.exited(0))
    let after = await waitUntil { asked().count == swapped + 1 && !m.remoteLoading }
    check("原来那个进程退出了：查一次", after)
    let offline = m.remote.doc!.sessions.first { !$0.attachable }!
    m.attachRemote(offline, readOnly: false)
    check("cca 说接不上的会话不接", m.remoteTerminal?.session.id == viewOnly.id)
    // 没量好大小就关掉
    let pending = m.remoteTerminal!
    m.closeRemoteTerminal()
    check("还没起进程就关掉：不起了", pending.status == .ended(0) && t.terminals.count == 3)
    m.attachRemote(far, readOnly: false)
    m.remoteTerminal?.resize(cols: 80, rows: 24)
    m.shutdown()
    check("退出 App：断开终端", t.terminals.last?.closed == 1)

    // 还没接上视图时输出太多：只留最近的
    let big = RemoteTerminal(session: far, readOnly: false) { args, cols, rows, onEvent in FakeTerminal(arguments: args, cols: cols, rows: rows, onEvent: onEvent) }
    big.begin(cols: 80, rows: 24)
    check("起不了两次", { big.begin(cols: 10, rows: 10); return big.status == .running }())
    let failing = RemoteTerminal(session: far, readOnly: false) { _, _, _, _ in throw TerminalUnsupported() }
    failing.begin(cols: 80, rows: 24)
    check("起不来：说原因", { if case .failed(let why) = failing.status { return !why.isEmpty }; return false }() && failing.hasEnded)

    // 换成了没有远程会话的旧版 cca：页面没了，开着的终端也断开
    let t2 = ScriptedTransport { request, _ in
        if request.arguments == ["_gui", "hello"] { return response(0, stdout: sw.failing ? hello.replacingOccurrences(of: #""remote": 1,"#, with: "") : hello) }
        if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
        if request.arguments.prefix(2) == ["remote", "list"] { return response(0, stdout: list) }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    sw.failing = false
    let m2 = makeModel(t2)
    await m2.start()
    m2.setMainVisible(true)
    m2.selection = .remoteSessions
    _ = await waitUntil { m2.remote.doc != nil }
    m2.attachRemote(far, readOnly: false)
    m2.remoteTerminal?.resize(cols: 80, rows: 24)
    sw.failing = true
    await m2.connect()
    check("cca 换成了没有远程会话的旧版：回总览，开着的终端断开", !m2.remoteAvailable && m2.selection == .office && m2.remoteTerminal == nil
          && t2.terminals.last?.closed == 1 && !m2.scheduler.remotePollingRunning)
    m2.shutdown()
}

/// 回归用：App 真正的伪终端（ProcessTransport.startTerminal）对真的 cca remote attach（regress.sh 10h）。
/// contract-tests --terminal-smoke cca路径 会话的id 停止文件 [先要看到的字 [要敲的一行 敲完要看到的字]]：
/// 接上去，等看到那几个字，敲一行，等看到回应；停止文件出现了就断开（和 App 的「断开」一样），打印 `exit=退出码 bytes=读到的字节数`。
/// 退出码：0 都对上了、断开后接入端的退出码是 0；70 没等到先要看到的字；71 没等到回应；72 断开后退出码不是 0；73 还没让断开它自己退出了。
func terminalSmoke(_ args: [String]) async -> Int32 {
    let env = ProcessInfo.processInfo.environment
    guard args.count >= 3 else { return 64 }
    let t = ProcessTransport(ccaPath: args[0], baseEnvironment: env, home: env["HOME"] ?? NSHomeDirectory(),
                             eventsDirectory: tmpRoot.appendingPathComponent("smoke-events"),
                             runsDirectory: tmpRoot.appendingPathComponent("smoke-runs"))
    defer { try? FileManager.default.removeItem(at: tmpRoot) }
    let log = PtyLog()
    guard let h = try? t.startTerminal(["remote", "attach", args[1]], cols: 90, rows: 28, onEvent: { log.add($0) }) else { return 65 }
    func wait(_ timeout: Double, _ cond: () -> Bool) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if cond() || log.exit != nil { return cond() }
            try? await Task.sleep(nanoseconds: 50_000_000)
        }
        return cond()
    }
    func report() { print("exit=\(log.exit.map(String.init) ?? "none") bytes=\(log.text.utf8.count)") }
    func fail(_ code: Int32) -> Int32 {
        FileHandle.standardError.write(Data(("看到的：" + log.text.suffix(400) + "\n").utf8))
        h.close()
        report()
        return code
    }
    if args.count >= 4, !(await wait(15) { log.text.contains(args[3]) }) { return fail(70) }
    if args.count >= 6 {
        h.resize(cols: 100, rows: 30)
        h.write(Data((args[4] + "\r").utf8))
        if !(await wait(15) { log.text.contains(args[5]) }) { return fail(71) }
    }
    _ = await wait(120) { FileManager.default.fileExists(atPath: args[2]) }
    if log.exit != nil {
        report()
        return 73
    }
    h.close()
    _ = await wait(10) { false }
    report()
    return log.exit == 0 ? 0 : 72
}

func ptyProcessTests() async {
    section("伪终端上的进程")
    func start(_ script: String, cols: Int = 100, rows: Int = 30) -> (PtyProcess, PtyLog)? {
        let log = PtyLog()
        var spec = ProcessSpec(executable: "/bin/sh", arguments: ["-c", script], environment: ["PATH": "/usr/bin:/bin", "TERM": "xterm-256color"], timeout: nil)
        spec.killGrace = 1
        guard let p = try? PtyProcess(spec: spec, cols: cols, rows: rows, onEvent: { log.add($0) }) else { return nil }
        return (p, log)
    }
    func wait(_ timeout: Double = 15, _ cond: () -> Bool) async -> Bool {   // 等到了就走；机器慢的时候（回归里和别的抢着跑）多等一会儿
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if cond() { return true }
            try? await Task.sleep(nanoseconds: 20_000_000)
        }
        return cond()
    }

    guard let (p, log) = start(#"[ -t 0 ] && [ -t 1 ] && [ -t 2 ] && echo istty; stty size; echo "term=$TERM"; printf 'ready\n'; read x; echo "got:$x"; stty size; read y; exit 7"#) else {
        check("开伪终端、起进程", false)
        return
    }
    check("标准输入、输出、错误都是终端；窗口大小是给的那个；环境照给",
          await wait { log.text.contains("ready") } && log.text.contains("istty") && log.text.contains("30 100") && log.text.contains("term=xterm-256color"), log.text)
    p.resize(cols: 120, rows: 40)
    p.write(Data("hello\n".utf8))
    check("写进去的是它的输入；改过的大小它读得到", await wait { log.text.contains("got:hello") && log.text.contains("40 120") }, log.text)
    p.write(Data("\n".utf8))
    check("退出码", await wait { log.exit != nil } && log.exit == 7, "\(String(describing: log.exit))")
    let seen = log.text
    p.write(Data("x".utf8))
    p.resize(cols: 10, rows: 10)
    p.close()
    try? await Task.sleep(nanoseconds: 200_000_000)
    check("退出以后再写、改大小、断开：什么都不发生（退出码、读到的都不变）", log.exit == 7 && log.text == seen)

    guard let (p2, log2) = start(#"trap 'echo bye; exit 0' TERM; echo ready; while :; do sleep 0.1; done"#) else {
        check("开伪终端、起进程", false)
        return
    }
    _ = await wait { log2.text.contains("ready") }
    let t0 = Date()
    p2.close()
    // killGrace 是 1 秒：退出码 0、说了 bye 就是自己收场的（被 SIGKILL 的是 137、来不及说）
    check("断开：SIGTERM，进程自己收场", await wait { log2.exit != nil } && log2.exit == 0 && log2.text.contains("bye"),
          "\(String(describing: log2.exit)) \(log2.text) \(Date().timeIntervalSince(t0))")

    guard let (p3, log3) = start(#"trap '' TERM; echo ready; while :; do sleep 0.1; done"#) else {
        check("开伪终端、起进程", false)
        return
    }
    _ = await wait { log3.text.contains("ready") }
    let t1 = Date()
    p3.close()
    check("不理 SIGTERM 的：过一会儿（killGrace）SIGKILL", await wait { log3.exit != nil } && log3.exit == 128 + SIGKILL && Date().timeIntervalSince(t1) >= 0.9,
          "\(String(describing: log3.exit)) \(Date().timeIntervalSince(t1))")

    guard let (_, log4) = start("head -c 3000000 /dev/zero | tr '\\0' 'x'; echo; echo END") else {
        check("开伪终端、起进程", false)
        return
    }
    check("输出多的一个字节不丢，读完了才报退出",
          await wait(20) { log4.exit != nil } && log4.text.hasSuffix("END\r\n") && log4.text.filter { $0 == "x" }.count == 3_000_000,
          "\(log4.text.count) \(String(describing: log4.exit))")

    let bad = ProcessSpec(executable: "/nonexistent/cca", arguments: [], environment: [:], timeout: nil)
    check("起不来：抛错，不留下伪终端", (try? PtyProcess(spec: bad, cols: 80, rows: 24, onEvent: { _ in })) == nil)
}
