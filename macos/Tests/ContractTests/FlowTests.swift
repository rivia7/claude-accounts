import Foundation

// 提问会话（PROTOCOL 4.6）：ask 事件、答案的格式、流程的状态机、真进程的答案管道。

private func ask(_ seq: Int, _ fields: [PromptField]) -> CcaEvent {
    .ask(AskEvent(seq: seq, q: "t.q", title: "", message: "", fields: fields))
}

func flowProtocolTests() async {
    section("提问会话：事件和环境")
    let line = #"{"v":1,"ev":"ask","seq":3,"q":"add.node","title":"T","message":"M","zz_future":1,"fields":[{"id":"v","kind":"choice","label":"出口节点","default":"2","zz":1,"options":[{"value":"1","label":"US-A","note":"已被 x 使用"},{"value":2,"label":"US-B"}]}]}"#
    let e = EventLog.parseLine(Data(line.utf8))?.ask
    check("ask 事件解码（选项的 value 是数字也收）", e?.seq == 3 && e?.q == "add.node" && e?.fields.first?.options.map(\.value) == ["1", "2"]
          && e?.fields.first?.defaultValue == "2" && e?.isDisplayable == true)
    check("类型是 ask", EventLog.parseLine(Data(line.utf8))?.type == .ask)
    check("没有序号的 ask 当作不认识的事件", EventLog.parseLine(Data(#"{"v":1,"ev":"ask","fields":[]}"#.utf8)) == .other("ask"))
    func displayable(_ fields: [PromptField]) -> Bool { AskEvent(seq: 1, q: "x", title: "", message: "", fields: fields).isDisplayable }
    check("不认识的字段类型不能显示", !displayable([PromptField(id: "a", kind: PromptKind(rawValue: "zz_new"))]))
    check("没有选项的选择题不能显示", !displayable([PromptField(id: "a", kind: .choice)]))
    check("没有字段的一页不能显示", !displayable([]))
    let env = CcaEnvironment.make(base: ["PATH": "/usr/bin", "CCA_PROMPT": "1"], home: "/h")
    check("平时不带 CCA_PROMPT（继承来的也去掉）", env["CCA_PROMPT"] == nil)
    let env2 = CcaEnvironment.make(base: ["PATH": "/usr/bin"], home: "/h", extra: ["CCA_PROMPT": "1"])
    check("extra 里不能设 CCA_PROMPT", env2["CCA_PROMPT"] == nil)
    check("提问会话才设 CCA_PROMPT=1", CcaEnvironment.make(base: ["PATH": "/usr/bin"], home: "/h", prompt: true)["CCA_PROMPT"] == "1")
    check("hello 的 prompt", (try? DocumentDecoder.decode(HelloDoc.self, from: Data(#"{"version":1,"kind":"hello","api":1,"prompt":1}"#.utf8)))?.prompt == 1)
    check("旧版 cca 的 hello 没有 prompt", (try? DocumentDecoder.decode(HelloDoc.self, from: Data(tinyHello.utf8)))?.prompt == nil)
    check("答案一行一个：文字去掉前后空白和换行", FlowModel.oneLine("  a\nb\r ", .text) == "a b")
    check("密文只去掉换行，空格原样", FlowModel.oneLine(" k e\ny\r\n", .secret) == " k ey")
    let handoff = Handoff.addAccount(prefix: ["codex"], product: "ChatGPT")
    check("添加账号能在 App 里问完，会话和编辑账号表不行",
          handoff.inApp && Handoff.addAPI().inApp && !Handoff.edit(prefix: []).inApp && !Handoff.session(prefix: [], key: "k", name: "n", directory: "/").inApp)
    check("删除账号、换 key 的命令", Handoff.remove(prefix: ["codex"], key: "k", name: "n").arguments == ["codex", "rm", "k"]
          && Handoff.apiKey(prefix: [], key: "k", name: "n").arguments == ["key", "k"])
}

@MainActor func flowModelTests() async {
    section("提问会话：流程的状态机")
    let h = Handoff.addAccount(prefix: [], product: "Claude")

    func make() -> (ScriptedSession, FlowModel, () -> [FlowModel.End]) {
        let s = ScriptedSession()
        var ends: [FlowModel.End] = []
        let f = FlowModel(handoff: h, session: s) { _, e in ends.append(e) }
        return (s, f, { ends })
    }

    do {
        let (s, f, ends) = make()
        check("一开始在等 cca", f.phase == .working)
        s.feed(ask(1, [PromptField(id: "email", kind: .text, label: "邮箱"), PromptField(id: "alias", kind: .text, optional: true),
                       PromptField(id: "key", kind: .secret)]))
        _ = await waitUntil { if case .asking = f.phase { return true }; return false }
        check("必填的没填：不能提交", !f.canSubmit)
        f.draft["email"] = " a@example.com "
        check("密文没填：不能提交", !f.canSubmit)
        f.setSecret("key", "SECRET-SENTINEL value")
        check("都填了：可以提交，可选的不用填", f.canSubmit)
        check("密文不在 draft 里", !f.draft.values.contains { $0.contains("SENTINEL") } && !"\(f.draft)".contains("SENTINEL"))
        f.submit()
        check("答案按字段顺序一行一个，带序号和字段 id", s.answers == [["1 email a@example.com", "1 alias ", "1 key SECRET-SENTINEL value"]], "\(s.answers)")
        check("交出去之后密文和草稿都清掉，等 cca", f.secret("key").isEmpty && f.draft.isEmpty && f.filledSecrets.isEmpty && f.phase == .working)
        // 校验不过：带着原因重问
        s.feed(ask(2, [PromptField(id: "email", kind: .text, value: "a@example.com", error: "重复了"), PromptField(id: "alias", kind: .text, optional: true),
                       PromptField(id: "key", kind: .secret, error: "太短")]))
        _ = await waitUntil { if case .asking(let e) = f.phase { return e.seq == 2 }; return false }
        check("重问时已填的带回来，密文要重新输入", f.draft["email"] == "a@example.com" && f.secret("key").isEmpty && !f.canSubmit)
        f.setSecret("key", "another-secret")
        f.submit()
        check("第二次的答案用新的序号", s.answers.last?.first == "2 email a@example.com")
        f.submit()
        check("等 cca 的时候再点也不会多发", s.answers.count == 2)
        s.feed(.result(ResultEvent(code: .added, route: nil, node: nil, port: nil, message: "已添加", tool: .claude, key: "k")))
        s.finish(0)
        _ = await waitUntil { !ends().isEmpty }
        check("退出码 0：成功，带结果", ends() == [.success(ResultEvent(code: .added, route: nil, node: nil, port: nil, message: "已添加", tool: .claude, key: "k"))])
    }
    do {
        let (s, f, ends) = make()
        s.feed(ask(1, [PromptField(id: "v", kind: .choice, defaultValue: "2", options: [PromptOption(value: "1", label: "A"), PromptOption(value: "2", label: "B")])]))
        _ = await waitUntil { if case .asking = f.phase { return true }; return false }
        check("选择题默认选中 cca 给的那一项，可以直接继续", f.draft["v"] == "2" && f.canSubmit)
        f.draft["v"] = "1"
        f.submit()
        check("选择题答 value", s.answers == [["1 v 1"]])
        s.feed(ask(2, [PromptField(id: "v", kind: .choice, optional: true, options: [PromptOption(value: "1", label: "A")])]))
        _ = await waitUntil { if case .asking(let e) = f.phase { return e.seq == 2 }; return false }
        check("可以不选的选择题：没有默认项，空着也能继续", f.draft["v"] == "" && f.canSubmit)
        f.submit()
        s.feed(ask(3, [PromptField(id: "v", kind: .multi, options: [PromptOption(value: "1", label: "A"), PromptOption(value: "3", label: "C"), PromptOption(value: "10", label: "J")], min: 2)]))
        _ = await waitUntil { if case .asking(let e) = f.phase { return e.seq == 3 }; return false }
        check("多选题没选够：不能继续", !f.canSubmit)
        f.toggle("v", "10")
        f.toggle("v", "3")
        check("多选的答案按编号排", f.draft["v"] == "3 10" && f.canSubmit)
        f.toggle("v", "3")
        check("再点一下取消选中", f.selected("v") == ["10"] && !f.canSubmit)
        f.toggle("v", "1")
        f.submit()
        check("多选答空格隔开的 value", s.answers.last == ["3 v 1 10"])
        s.feed(ask(4, [PromptField(id: "v", kind: .confirm, defaultValue: "n", severity: .warn)]))
        _ = await waitUntil { if case .asking(let e) = f.phase { return e.seq == 4 }; return false }
        f.confirm(true)
        check("是否题答 y", s.answers.last == ["4 v y"])
        s.feed(.fail(FailEvent(code: .syncFailed, message: "路由器同步没完成")))
        s.feed(.result(ResultEvent(code: .added, route: nil, node: nil, port: nil, message: "已添加")))
        s.finish(0)
        _ = await waitUntil { f.phase != .working }
        check("退出码 0 但有 fail：做完了，要提醒", f.phase == .warned(message: "已添加", warnings: ["路由器同步没完成"]) && ends().isEmpty)
    }
    do {
        let (s, f, ends) = make()
        s.feed(.fail(FailEvent(code: .appRunning, message: "还开着")))
        s.finish(1, stderr: "详情一\n详情二\n")
        _ = await waitUntil { f.phase != .working }
        if case .failed(let x) = f.phase {
            check("失败：原因、码、stderr 的最后几行", x.message == "还开着" && x.code == .appRunning && x.stderrTail.contains("详情二") && ends().isEmpty)
        } else {
            check("失败页", false, "\(f.phase)")
        }
    }
    do {
        let (s, _, ends) = make()
        let t = TerminalEvent(code: .routerSetup, argv: ["router", "setup"], display: nil, message: "配置路由器")
        s.feed(.terminal(t))
        s.finish(1)
        _ = await waitUntil { !ends().isEmpty }
        check("要在终端里做：交给 App 弹终端交接", ends() == [.terminal(t)])
    }
    do {
        let (s, _, ends) = make()
        s.feed(.fail(FailEvent(code: .cancelled, message: "已取消")))
        s.finish(143)
        _ = await waitUntil { !ends().isEmpty }
        check("cca 说取消了：不算失败", ends() == [.cancelled])
    }
    do {
        let (s, f, ends) = make()
        s.feed(ask(1, [PromptField(id: "x", kind: .secret)]))
        _ = await waitUntil { if case .asking = f.phase { return true }; return false }
        f.setSecret("x", "SECRET")
        f.cancel()
        check("取消：关掉答案管道，马上结束，密文清掉", s.closed == 1 && ends() == [.cancelled] && f.secret("x").isEmpty)
        f.cancel()
        s.finish(143)
        try? await Task.sleep(nanoseconds: 100_000_000)
        check("取消之后的结果不再处理", ends() == [.cancelled] && s.closed == 1)
    }
    do {
        let (s, f, ends) = make()
        s.feed(ask(1, [PromptField(id: "x", kind: PromptKind(rawValue: "zz_new"))]))
        _ = await waitUntil { s.closed == 1 }
        check("显示不了的一页：关掉管道，不能漏掉问题", s.closed == 1 && f.phase == .working)
        s.feed(.fail(FailEvent(code: .cancelled, message: "已取消")))
        s.finish(143)
        _ = await waitUntil { f.phase != .working }
        if case .failed(let x) = f.phase {
            check("显示不了时走失败页（上面有改在终端里完成）", x.message.contains("显示不了") && ends().isEmpty)
        } else {
            check("显示不了时的失败页", false, "\(f.phase)")
        }
    }
    do {
        let (s, f, _) = make()
        f.stop()
        check("等太久可以停掉", s.stopped == 1)
    }

    section("提问会话：AppModel")
    let helloPrompt = #"{"version":1,"kind":"hello","api":1,"prompt":1,"cca_version":"9.9.9","installed":true,"home":"/nonexistent/.cca","tools":{"claude":{"enabled":true,"bin":null},"codex":{"enabled":false,"bin":null}},"gui":{"app":"/x","app_version":null,"staged_version":null}}"#
    let state = #"{"version":1,"kind":"state","generated_at":1794042000.0,"cca_version":"9.9.9","errors":[],"notices":[],"config":{},"routing":{},"tools":[{"id":"claude","label":"Claude","app_label":"Claude App","cli_prefix":[],"enabled":true,"accounts":[{"key":"k1","id":"k1","alias":"","email":"k1@example.com","name":"k1","label":"k1","kind":"oauth","login":"logged_out","can":{}}]}]}"#
    func scripted(_ hello: String, _ sessions: [ScriptedSession]) -> (ScriptedTransport, AppModel) {
        let t = ScriptedTransport { r, _ in
            if r.arguments == ["_gui", "hello"] { return response(0, stdout: hello) }
            if r.arguments == ["_gui", "state"] { return response(0, stdout: state) }
            return response(0)
        }
        t.sessions = sessions
        return (t, makeModel(t))
    }
    do {
        let (t, m) = scripted(tinyHello, [ScriptedSession()])
        await m.connect()
        m.run(.addAPI())
        check("旧版 cca（hello 没有 prompt）：走终端", t.started.isEmpty && m.flow == nil && m.toast?.text == "找不到 cca")
    }
    do {
        let s = ScriptedSession()
        let (t, m) = scripted(helloPrompt, [s])
        await m.connect()
        m.run(.session(prefix: [], key: "k1", name: "n", directory: "/tmp"))
        check("账号的会话一定走终端", t.started.isEmpty && m.flow == nil)
        m.run(.addAccount(prefix: [], product: "Claude"))
        check("支持时开提问会话，弹层是这个流程", t.started == [["add"]] && m.flow != nil && {
            if case .flow = m.sheet { return true }; return false }())
        let info = FailureInfo(title: "别的失败", failure: Failure(reason: .command, code: nil, message: "x", stderrTail: "", exitCode: 1, tool: nil, key: nil))
        m.present(.failure(info))
        check("流程问到一半时别的弹层排队，不顶掉它", { if case .flow = m.sheet { return true }; return false }())
        m.run(.addAPI())
        check("同一时间只开一个流程", t.started.count == 1)
        s.feed(.result(ResultEvent(code: .added, route: nil, node: nil, port: nil, message: "已添加 k1", tool: .claude, key: "k1")))
        s.finish(0)
        _ = await waitUntil { m.flow == nil }
        check("结束后关掉，排队的弹层出来，提示 cca 的原话", m.flow == nil && m.sheet == .failure(info) && m.toast?.text == "已添加 k1")
        _ = await waitUntil { m.selection == .account(AccountRef(tool: .claude, key: "k1")) }
        check("加好之后选中这个账号", m.selection == .account(AccountRef(tool: .claude, key: "k1")))
    }
    do {
        let s = ScriptedSession()
        let (_, m) = scripted(helloPrompt, [s])
        await m.connect()
        m.run(.addAPI())
        let t = TerminalEvent(code: .routerSetup, argv: ["router", "setup"], display: "cca router setup", message: "配置路由器")
        s.feed(.terminal(t))
        s.finish(1)
        _ = await waitUntil { m.flow == nil }
        check("流程要转终端：弹终端交接", m.sheet == .handoff(Handoff(event: t)))
    }
    do {
        let s = ScriptedSession()
        let (_, m) = scripted(helloPrompt, [s])
        await m.connect()
        m.run(.addAPI())
        s.feed(.result(ResultEvent(code: .added, route: nil, node: nil, port: nil, message: "已添加 k1", tool: .claude, key: "k1")))
        s.feed(.fail(FailEvent(code: .syncFailed, message: "路由器同步没完成")))
        s.finish(0)
        let flow = m.flow
        _ = await waitUntil { if let f = flow, case .warned = f.phase { return true }; return false }
        check("做完了但要提醒：留在弹层上", m.flow != nil)
        if let flow { m.closeFlow(flow) }
        _ = await waitUntil { m.selection == .account(AccountRef(tool: .claude, key: "k1")) }
        check("提醒页关掉之后也选中新账号", m.flow == nil && m.selection == .account(AccountRef(tool: .claude, key: "k1")))
    }
    do {
        let s = ScriptedSession()
        let (_, m) = scripted(helloPrompt, [s])
        await m.connect()
        m.run(.addAPI())
        s.feed(.fail(FailEvent(code: .error, message: "没存进钥匙串")))
        s.finish(1)
        let flow = m.flow
        _ = await waitUntil { if let f = flow, case .failed = f.phase { return true }; return false }
        check("失败时留在弹层上", m.flow != nil)
        if let flow { m.closeFlow(flow) }
        check("点“好”才关", m.flow == nil && m.sheet == nil)
    }
    do {
        let (t, m) = scripted(helloPrompt, [])
        await m.connect()
        m.run(.addAPI())
        check("开不了提问会话：退回终端", t.started == [["add", "api"]] && m.flow == nil && m.toast?.text == "找不到 cca")   // 测试里没有 cca 的路径，走到 runInTerminal 就说找不到
    }
    do {
        let m = makeModel(fixtureTransport())
        await m.connect()
        m.run(.addAccount(prefix: [], product: "Claude"))
        let flow = m.flow
        _ = await waitUntil { if case .asking = flow?.phase { return true }; return false }
        if case .asking(let e) = flow?.phase {
            check("演示数据也能走流程", e.q == "add.basic" && e.fields.count == 2)
        } else {
            check("演示数据也能走流程", false)
        }
    }
}

/// 回归用：App 真正的答案管道（ProcessTransport）对真的 cca（regress.sh 10h）。
/// contract-tests --session-smoke cca路径 答案表.json 命令…：按题号回答，退出码就是 cca 的。
func sessionSmoke(_ args: [String]) async -> Int32 {
    let env = ProcessInfo.processInfo.environment
    guard args.count >= 3, let data = try? Data(contentsOf: URL(fileURLWithPath: args[1])),
          let table = (try? JSONSerialization.jsonObject(with: data)) as? [String: [[String: String]]] else { return 64 }
    let t = ProcessTransport(ccaPath: args[0], baseEnvironment: env, home: env["HOME"] ?? NSHomeDirectory(),
                             eventsDirectory: tmpRoot.appendingPathComponent("smoke-events"),
                             runsDirectory: tmpRoot.appendingPathComponent("smoke-runs"))
    defer { try? FileManager.default.removeItem(at: tmpRoot) }
    guard let s = try? t.startSession(Array(args.dropFirst(2))) else { return 65 }
    var used: [String: Int] = [:]
    for await u in s.updates {
        switch u {
        case .event(.ask(let e)):
            let i = used[e.q, default: 0]
            used[e.q] = i + 1
            guard let rounds = table[e.q], i < rounds.count else {
                s.closeAnswers()
                continue
            }
            s.answer(e.fields.map { "\(e.seq) \($0.id) \(rounds[i][$0.id] ?? "")" })
        case .finished(let r):
            FileHandle.standardError.write(r.stderr)
            return r.exitCode
        default:
            break
        }
    }
    return 66
}

func flowProcessTests() async {
    section("提问会话：真的进程")
    let fm = FileManager.default
    let dir = tmpRoot.appendingPathComponent("flow-session")
    try? fm.createDirectory(at: dir, withIntermediateDirectories: true)
    let fake = dir.appendingPathComponent("cca")
    let out = dir.appendingPathComponent("out.txt")
    let script = #"""
    #!/bin/bash
    { : <&3; } 2>/dev/null || exit 66   # 和 cca 开头判断提问会话的写法一样
    { echo "ARGS:$*"; env; [ -t 0 ] && echo STDIN-IS-TTY; head -c 1 2>/dev/null | wc -c | sed 's/^/STDIN-BYTES:/'; } >> "$CCA_TEST_OUT"
    ask() { printf '%s\n' "{\"v\":1,\"ev\":\"ask\",\"seq\":$1,\"q\":\"x.q\",\"fields\":[{\"id\":\"k\",\"kind\":\"secret\",\"label\":\"K\"}]}" >> "$CCA_EVENTS"; }
    case "$1" in
      roundtrip)
        ask 1
        IFS= read -r line <&3 || exit 7
        printf 'GOT:%s\n' "$line" >> "$CCA_TEST_OUT.answers"
        echo '进度：正在处理' >&2
        ask 2
        IFS= read -r line <&3 || exit 7
        printf 'GOT:%s\n' "$line" >> "$CCA_TEST_OUT.answers"
        printf '%s\n' '{"v":1,"ev":"result","code":"added","tool":"claude","key":"k1","message":"完成"}' >> "$CCA_EVENTS"
        echo 输出 ;;
      eof)
        ask 1
        IFS= read -r line <&3 && exit 8
        exit 7 ;;
      quick) exit 5 ;;
      hang) ask 1; trap 'exit 9' TERM; sleep 30 & wait ;;
    esac
    """#
    fm.createFile(atPath: fake.path, contents: Data(script.utf8), attributes: [.posixPermissions: 0o755])
    let events = dir.appendingPathComponent("events"), runs = dir.appendingPathComponent("runs")
    let t = ProcessTransport(ccaPath: fake.path, baseEnvironment: ["PATH": "/usr/bin:/bin", "CCA_TEST_OUT": out.path, "CCA_TTY": "/x", "CCA_PROMPT": "0"],
                             home: tmpRoot.path, eventsDirectory: events, runsDirectory: runs)

    /// 收集这个会话的进展，遇到 ask 按 onAsk 回答。
    func drive(_ s: CcaSession, onAsk: (AskEvent) -> Void) async -> (asks: [AskEvent], outputs: [String], response: CcaResponse?) {
        var asks: [AskEvent] = [], outputs: [String] = [], final: CcaResponse?
        for await u in s.updates {
            switch u {
            case .event(.ask(let e)):
                asks.append(e)
                onAsk(e)
            case .output(let line): outputs.append(line)
            case .finished(let r): final = r
            case .event: break
            }
        }
        return (asks, outputs, final)
    }

    let secret = "SECRET-SENTINEL-session 'x' value"
    let s1: CcaSession
    do {
        s1 = try t.startSession(["roundtrip"])
    } catch {
        check("开提问会话", false, "\(error)")
        return
    }
    let r1 = await drive(s1) { e in s1.answer(["\(e.seq) k \(secret)"]) }
    let got = (try? String(contentsOf: URL(fileURLWithPath: out.path + ".answers"), encoding: .utf8)) ?? ""
    let log = (try? String(contentsOf: out, encoding: .utf8)) ?? ""
    check("边问边答：cca 收到两页答案", got == "GOT:1 k \(secret)\nGOT:2 k \(secret)\n", got)
    check("事件按顺序交出来，退出码、stdout 都在", r1.asks.map(\.seq) == [1, 2] && r1.response?.exitCode == 0 && r1.response?.stdoutText == "输出\n")
    check("进度：最新的一行输出", r1.outputs.contains("进度：正在处理"), "\(r1.outputs)")
    check("环境：CCA_PROMPT=1，没有 CCA_TTY；stdin 是空的、不是终端", log.contains("CCA_PROMPT=1\n") && !log.contains("CCA_PROMPT=0") && !log.contains("CCA_TTY=")
          && !log.contains("STDIN-IS-TTY") && log.contains("STDIN-BYTES:       0") && log.contains("ARGS:roundtrip\n"), log)
    check("密文不进参数和环境", !log.contains("SENTINEL"))
    let runFiles = (try? fm.contentsOfDirectory(atPath: runs.path)) ?? [], eventFiles = (try? fm.contentsOfDirectory(atPath: events.path)) ?? []
    check("结束后输出文件和事件文件都删了（里面没有残留）", runFiles.isEmpty && eventFiles.isEmpty, "\(runFiles) \(eventFiles)")

    if let s2 = try? t.startSession(["eof"]) {
        let r2 = await drive(s2) { _ in s2.closeAnswers() }
        check("关掉答案管道：cca 读到 EOF", r2.response?.exitCode == 7)
    } else {
        check("开提问会话（eof）", false)
    }
    if let s3 = try? t.startSession(["quick"]) {
        let r3 = await drive(s3) { _ in }
        check("进程先退出了", r3.response?.exitCode == 5)
        s3.answer(["1 k late"])
        s3.closeAnswers()
        s3.closeAnswers()
        s3.stop()
        try? await Task.sleep(nanoseconds: 100_000_000)
        check("进程退出后再写答案不会把 App 杀掉（SIGPIPE）", true)
    } else {
        check("开提问会话（quick）", false)
    }
    if let s4 = try? t.startSession(["hang"]) {
        let r4 = await drive(s4) { _ in s4.stop() }
        check("步骤卡住时可以停掉（给整个进程组发 SIGTERM）", r4.response?.exitCode == 9, "\(String(describing: r4.response?.exitCode))")
    } else {
        check("开提问会话（hang）", false)
    }
}
