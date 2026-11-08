import Foundation

func textTests() {
    section("文字")
    check("span", Texts.span(30) == "不到 1 分钟" && Texts.span(25 * 60) == "25 分钟" && Texts.span(8010) == "2 小时 13 分"
          && Texts.span(7200) == "2 小时" && Texts.span(90000) == "1 天 1 小时" && Texts.span(86400 * 2) == "2 天")
    check("ago", Texts.ago(10) == "1 分钟" && Texts.ago(2400) == "40 分钟" && Texts.ago(7300) == "2 小时" && Texts.ago(259200) == "3 天")
    check("agoPhrase", Texts.agoPhrase(5) == "刚刚" && Texts.agoPhrase(240) == "4 分钟前")
    var utc = Calendar(identifier: .gregorian)
    utc.timeZone = TimeZone(identifier: "UTC")!
    var sh = Calendar(identifier: .gregorian)
    sh.timeZone = TimeZone(identifier: "Asia/Shanghai")!
    let now = 1794042000.0
    check("一天以内写还有多久", Texts.resetText(epoch: now + 8010, now: now, calendar: utc) == "2 小时 13 分后重置")
    check("再远写星期几、几点", Texts.resetText(epoch: 1794297600, now: now, calendar: utc) == "周二 08:00 重置"
          && Texts.resetText(epoch: 1794297600, now: now, calendar: sh) == "周二 16:00 重置",
          Texts.resetText(epoch: 1794297600, now: now, calendar: utc))
    check("周日", Texts.resetText(epoch: 1794470400 + 86400 * 3, now: now, calendar: utc).hasPrefix("周日"),
          Texts.resetText(epoch: 1794470400 + 86400 * 3, now: now, calendar: utc))
    check("已经过了", Texts.resetText(epoch: now - 1, now: now) == "已重置")
    check("百分比", Texts.percent(12.5) == "13%" && Texts.percent(0) == "0%" && Texts.percent(99.4) == "99%")
    check("颜色档", UsageLevel(percent: 69.9) == .normal && UsageLevel(percent: 70) == .warning && UsageLevel(percent: 89.9) == .warning
          && UsageLevel(percent: 90) == .critical && UsageLevel(percent: 120) == .critical)
    check("登录状态", Texts.login(.emailMismatch, loginEmail: "eve@example.com") == "登录的是 eve@example.com"
          && Texts.login(LoginState(rawValue: "zz"), loginEmail: nil) == "登录状态未知")
    check("步骤", Texts.step(.open, appLabel: "ChatGPT App") == "打开 ChatGPT App…" && Texts.step(.sync, appLabel: "") == "同步路由器…")
    check("产品名", Texts.product(.codex) == "ChatGPT" && Texts.product(.claude) == "Claude")
    check("不认识的槽位状态", Texts.slotState(SlotState(rawValue: "zz")) == "zz")
    check("直连", Texts.route(.direct) == "直连" && Texts.via(.direct) == "直连" && Texts.via(.router) == "经路由器"
          && Texts.via(nil) == nil && Route.direct.isKnown)
    check("账号的登录状态读不了", StateErrorCode.accountUnreadable.isKnown && Texts.stateError(try! JSONDecoder().decode(
        StateIssue.self, from: Data(#"{"code":"account_unreadable","tool":"claude","message":""}"#.utf8))).contains("当作没登录"))
    check("cca 不会用的失败码不在已知列表里", !FailCode(rawValue: "drift").isKnown)

    section("补救")
    check("port_closed → 同步路由器", Remedy.forCode(.portClosed, tool: .claude, key: "k") == .routerSync)
    check("table_invalid → cca edit", Remedy.forCode(.tableInvalid, tool: .claude, key: nil) == .handoff(.edit(prefix: [])))
    check("Codex 的 table_invalid → cca codex edit", Remedy.forCode(.tableInvalid, tool: .codex, key: nil) == .handoff(.edit(prefix: ["codex"])))
    check("node_missing → cca edit", Remedy.forCode(.nodeMissing, tool: .claude, key: "k") == .handoff(.edit(prefix: [])))
    check("no_router → cca router setup", Remedy.forCode(.noRouter, tool: .claude, key: nil) == .handoff(.routerSetup))
    check("local_unavailable → cca local setup", Remedy.forCode(.localUnavailable, tool: .claude, key: nil) == .handoff(.localSetup))
    if case .handoff(let h) = Remedy.forCode(.notLoggedIn, tool: .codex, key: "f@example.com") {
        check("not_logged_in → cca codex login KEY", h.arguments == ["codex", "login", "f@example.com"])
    } else {
        check("not_logged_in → cca codex login KEY", false)
    }
    if case .handoff(let h) = Remedy.forCode(.loginMismatch, tool: .claude, key: "工作(bob@example.com)") {
        check("login_mismatch → cca login KEY", h.arguments == ["login", "工作(bob@example.com)"]
              && h.display == "cca login '工作(bob@example.com)'")
    } else {
        check("login_mismatch → cca login KEY", false)
    }
    check("app_missing → 下载页（ChatGPT App 没有固定的下载地址：不给按钮，照 cca 的说明来）",
          Remedy.forCode(.appMissing, tool: .claude, key: nil) == .openURL("https://claude.ai/download")
          && Remedy.forCode(.appMissing, tool: .codex, key: nil) == .none)
    check("其它没有补救", Remedy.forCode(.lowPurity, tool: .claude, key: nil) == Remedy.none
          && Remedy.forCode(FailCode(rawValue: "zz"), tool: .claude, key: nil) == Remedy.none)

    section("菜单栏标记、刷新判断")
    func entry(_ p: Double, fetched: Double, stale: Bool = false, resets: Double = 1794050010) -> UsageEntry {
        let j = #"{"key":"k","source":"cache","fetched_epoch":\#(fetched),"stale":\#(stale),"five_hour":{"utilization":\#(p),"resets_epoch":\#(resets),"reset":false}}"#
        return try! JSONDecoder().decode(UsageEntry.self, from: Data(j.utf8))
    }
    check("待同步要标记", Attention.needed(sync: .pending, live: nil, usages: [], now: now))
    check("≥90% 要标记", Attention.needed(sync: .ok, live: nil, usages: [entry(92, fetched: now)], now: now))
    check("89% 不标记", !Attention.needed(sync: .ok, live: nil, usages: [entry(89, fetched: now)], now: now))
    check("已经重置的不算", !Attention.needed(sync: .ok, live: nil, usages: [entry(95, fetched: now - 99999, resets: now - 10)], now: now))
    if let l = fixture("live_router.json").flatMap({ try? DocumentDecoder.decode(LiveDoc.self, from: $0) }) {
        check("drift 要标记", Attention.needed(sync: .ok, live: l, usages: [], now: now))
    }
}

func shellTests() async {
    section("shellQuote")
    check("普通", shellQuote("abc") == "'abc'")
    check("单引号", shellQuote("it's") == #"'it'\''s'"#)
    check("空字符串", shellQuote("") == "''")
    check("按需加引号", shellQuoteIfNeeded("alice@example.com") == "alice@example.com" && shellQuoteIfNeeded("a b") == "'a b'"
          && shellQuoteIfNeeded("工作(bob@example.com)") == "'工作(bob@example.com)'" && shellQuoteIfNeeded("") == "''")
    check("显示的命令", displayCommand(["login", "alice@example.com"]) == "cca login alice@example.com")
    let samples = ["abc", "it's", "", "a b  c", "$HOME", "`date`", "\"q\"", "工作(bob@example.com)", "a\nb", "\\", "'", "''", "-n", "!x"]
    var ok = true
    var bad = ""
    for s in samples {
        let spec = ProcessSpec(executable: "/bin/sh", arguments: ["-c", "printf %s " + shellQuote(s)], environment: ["PATH": "/usr/bin:/bin"], timeout: 10)
        let r = try? await CcaProcess.run(spec)
        if r?.stdoutText != s {
            ok = false
            bad += "[\(s)→\(r?.stdoutText ?? "nil")]"
        }
    }
    check("sh 里原样还原", ok, bad)
}

func environmentTests() {
    section("环境")
    let base = ["PATH": "/usr/bin:/bin", "CCA_TTY": "/tmp/answers", "CCA_YES": "1", "CCA_GRANT": "old", "CCA_EVENTS": "/tmp/old",
                "CCA_RO": "1", "CCA_SKIP_CHECK": "1", "KEEP": "1", "HOME": "/Users/me"]
    let env = CcaEnvironment.make(base: base, home: "/Users/me", eventsPath: "/tmp/e.jsonl", grants: ["router_reload:OpenClash", "low_purity:203.0.113.20"],
                                  extra: ["CCA_ROUTE": "local", "CCA_YES": "1", "CCA_TTY": "/x", "CCA_SKIP_CHECK": "1"])
    check("去掉 CCA_TTY / CCA_YES / CCA_SKIP_CHECK / CCA_RO", env["CCA_TTY"] == nil && env["CCA_YES"] == nil && env["CCA_SKIP_CHECK"] == nil && env["CCA_RO"] == nil)
    check("CCA_GRANT 用逗号连起来", env["CCA_GRANT"] == "router_reload:OpenClash,low_purity:203.0.113.20")
    check("CCA_EVENTS", env["CCA_EVENTS"] == "/tmp/e.jsonl")
    check("CCA_GUI、NO_COLOR", env["CCA_GUI"] == "1" && env["NO_COLOR"] == "1")
    check("别的照样继承、extra 加上", env["KEEP"] == "1" && env["CCA_ROUTE"] == "local")
    check("PATH 后面补上", env["PATH"] == "/usr/bin:/bin:/Users/me/.local/bin:/opt/homebrew/bin:/usr/local/bin", env["PATH"] ?? "")
    let env2 = CcaEnvironment.make(base: ["PATH": "/opt/homebrew/bin:/usr/bin", "CCA_GRANT": "x", "CCA_EVENTS": "y"], home: "/Users/me")
    check("已有的不重复补", env2["PATH"] == "/opt/homebrew/bin:/usr/bin:/Users/me/.local/bin:/usr/local/bin", env2["PATH"] ?? "")
    check("没给就不带 CCA_GRANT / CCA_EVENTS", env2["CCA_GRANT"] == nil && env2["CCA_EVENTS"] == nil)
    let env3 = CcaEnvironment.make(base: [:], home: "/h")
    check("没有 PATH 时用默认的", env3["PATH"] == "/usr/bin:/bin:/usr/sbin:/sbin:/h/.local/bin:/opt/homebrew/bin:/usr/local/bin" && env3["HOME"] == "/h")
}

func locatorTests() {
    section("找 cca")
    let home = tmpRoot.appendingPathComponent("home-locator")
    let fm = FileManager.default
    try? fm.createDirectory(at: home.appendingPathComponent(".local/bin"), withIntermediateDirectories: true)
    try? fm.createDirectory(at: home.appendingPathComponent(".cca/bin"), withIntermediateDirectories: true)
    try? fm.createDirectory(at: home.appendingPathComponent("dev"), withIntermediateDirectories: true)
    func put(_ rel: String) {
        let p = home.appendingPathComponent(rel).path
        fm.createFile(atPath: p, contents: Data("#!/bin/sh\n".utf8), attributes: [.posixPermissions: 0o755])
    }
    check("都没有", CcaLocator.locate(environment: [:], home: home.path) == nil)
    put(".cca/bin/cca")
    check("只有 ~/.cca/bin/cca", CcaLocator.locate(environment: [:], home: home.path) == home.appendingPathComponent(".cca/bin/cca").path)
    put(".local/bin/cca")
    check("优先 ~/.local/bin/cca", CcaLocator.locate(environment: [:], home: home.path) == home.appendingPathComponent(".local/bin/cca").path)
    put("dev/cca")
    let dev = home.appendingPathComponent("dev/cca").path
    check("开发模式 CCA_GUI_CCA 最优先", CcaLocator.locate(environment: ["CCA_GUI_CCA": dev], home: home.path) == dev)
    check("CCA_GUI_CCA 不能执行时不用", CcaLocator.locate(environment: ["CCA_GUI_CCA": home.appendingPathComponent("nope").path], home: home.path)
          == home.appendingPathComponent(".local/bin/cca").path)
    try? fm.setAttributes([.posixPermissions: 0o644], ofItemAtPath: home.appendingPathComponent(".local/bin/cca").path)
    check("不能执行的跳过", CcaLocator.locate(environment: [:], home: home.path) == home.appendingPathComponent(".cca/bin/cca").path)
    check("目录不算", !CcaLocator.isExecutableFile(home.appendingPathComponent("dev").path))
}

func watcherTests() async {
    section("看 ~/.cca")
    let homes = ["/Users/me/.cca"]
    let cases: [(String, CcaWatcher.Trigger?)] = [
        ("/Users/me/.cca/accounts.json", .state), ("/Users/me/.cca/codex.json", .state), ("/Users/me/.cca/config.json", .state),
        ("/Users/me/.cca/accounts/bob-1/usage.json", .state), ("/Users/me/.cca/codex/accounts/frank-1/health.json", .state),
        ("/Users/me/.cca/policy/policy.json", .state), ("/Users/me/.cca/bin/cca", .hello),
        ("/Users/me/.cca/accounts/bob-1/chrome/Default/usage.json", nil), ("/Users/me/.cca/accounts/bob-1/desktop/health.json", nil),
        ("/Users/me/.cca/chrome/x/config.json", nil), ("/Users/me/.cca/accounts.json.tmp123", nil), ("/Users/me/.cca/leak.json", nil),
        ("/Users/me/.cca/usage.json", nil), ("/Users/me/.ccaX/accounts.json", nil), ("/Users/me/.cca", nil),
        ("/Users/other/.cca/accounts.json", nil), ("/Users/me/.cca/bin/cca.new", nil), ("/Users/me/.cca/policy/nodes.json", nil),
        ("/Users/me/.cca/cm-abc123", nil), ("/Users/me/.cca/backup/clash-verge-x.yaml", nil),
        ("/Users/me/.cca/core/config.yaml", .core), ("/Users/me/.cca/core/nodes.json", nil), ("/Users/me/.cca/core/cache.db", nil),
        ("/Users/me/.cca/core/config.yaml.123.tmp", nil), ("/Users/me/.cca/accounts/a/core/config.yaml", nil),
        // 命令行会话的登记：出现、消失都要重新读 state（办公室里谁在干活）
        ("/Users/me/.cca/accounts/bob-1/cli-123.json", .state), ("/Users/me/.cca/codex/accounts/frank-1/cli-9.json", .state),
        ("/Users/me/.cca/accounts/bob-1/cli-123.json.cca-9.tmp", nil), ("/Users/me/.cca/accounts/bob-1/claude/cli-1.json", nil),
        ("/Users/me/.cca/accounts/bob-1/cli-x.json", nil), ("/Users/me/.cca/accounts/bob-1/cli-.json", nil),
        ("/Users/me/.cca/cli-1.json", nil),
    ]
    var bad: [String] = []
    for (p, want) in cases where CcaWatcher.classify(path: p, homes: homes) != want { bad.append(p) }
    check("路径过滤", bad.isEmpty, bad.joined(separator: ", "))
    check("合并时 hello > core > state", CcaWatcher.Trigger.hello.rank > CcaWatcher.Trigger.core.rank
          && CcaWatcher.Trigger.core.rank > CcaWatcher.Trigger.state.rank)
    check("解析过链接的写法也认", CcaWatcher.classify(path: "/private/x/.cca/config.json", homes: ["/Users/me/.cca", "/private/x/.cca"]) == .state)

    // 真的 FSEvents：写 accounts.json 触发，写 chrome/ 下面的不触发
    let dir = tmpRoot.appendingPathComponent("watch-home")
    try? FileManager.default.createDirectory(at: dir.appendingPathComponent("accounts/a/chrome"), withIntermediateDirectories: true)
    let sink = TriggerSink()
    let w = CcaWatcher(home: dir.path, debounce: 0.3, latency: 0.1) { sink.add($0) }
    w.start()
    try? await Task.sleep(nanoseconds: 700_000_000)
    try? Data("x".utf8).write(to: dir.appendingPathComponent("accounts/a/chrome/usage.json"))
    try? await Task.sleep(nanoseconds: 1_500_000_000)
    check("chrome/ 下面的变化不触发", sink.items.isEmpty, "\(sink.items)")
    for i in 0..<3 {
        try? Data("{\"n\": \(i)}".utf8).write(to: dir.appendingPathComponent("accounts.json"), options: .atomic)
        try? await Task.sleep(nanoseconds: 50_000_000)
    }
    var waited = 0
    while sink.items.isEmpty && waited < 60 {
        try? await Task.sleep(nanoseconds: 100_000_000)
        waited += 1
    }
    try? await Task.sleep(nanoseconds: 800_000_000)
    check("accounts.json 变了触发一次 state（合并）", sink.items == [.state], "\(sink.items)")
    try? FileManager.default.createDirectory(at: dir.appendingPathComponent("bin"), withIntermediateDirectories: true)
    try? Data("#!/bin/sh\n".utf8).write(to: dir.appendingPathComponent("bin/cca"))
    waited = 0
    while sink.items.count < 2 && waited < 60 {
        try? await Task.sleep(nanoseconds: 100_000_000)
        waited += 1
    }
    check("bin/cca 变了触发 hello", sink.items.last == .hello, "\(sink.items)")
    w.stop()
}

func handoffTests() async {
    section("终端交接")
    let dir = tmpRoot.appendingPathComponent("handoff")
    let fake = tmpRoot.appendingPathComponent("fake cca's dir/cca")
    try? FileManager.default.createDirectory(at: fake.deletingLastPathComponent(), withIntermediateDirectories: true)
    FileManager.default.createFile(atPath: fake.path, contents: Data("#!/bin/sh\nfor a in \"$@\"; do printf '<%s>\\n' \"$a\"; done\nexit 7\n".utf8),
                                   attributes: [.posixPermissions: 0o755])
    let args = ["login", "工作(bob@example.com)", "it's", "$HOME", "a b", "", "`x`"]
    let h = Handoff(arguments: args, title: "登录")
    guard let url = try? TerminalHandoff.write(h, ccaPath: fake.path, directory: dir) else {
        check("写 .command 文件", false)
        return
    }
    let attrs = try? FileManager.default.attributesOfItem(atPath: url.path)
    check("写 .command 文件（755）", url.pathExtension == "command" && (attrs?[.posixPermissions] as? NSNumber)?.intValue == 0o755)
    let text = (try? String(contentsOf: url, encoding: .utf8)) ?? ""
    check("bash、补 PATH", text.hasPrefix("#!/bin/bash\n") && text.contains("/opt/homebrew/bin") && text.contains(".local/bin"))
    check("去掉 CCA_TTY 等变量", text.contains("unset CCA_TTY CCA_YES CCA_GRANT CCA_EVENTS CCA_RO CCA_SKIP_CHECK"))
    let spec = ProcessSpec(executable: "/bin/bash", arguments: [url.path], environment: ["PATH": "/usr/bin:/bin", "HOME": "/nonexistent"], timeout: 20)
    let r = try? await CcaProcess.run(spec)
    let printed = (r?.stdoutText ?? "").split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        .filter { $0.hasPrefix("<") }
    check("参数原样传给 cca", printed == args.map { "<\($0)>" }, "\(printed)")
    check("退出码照传", r?.exitCode == 7)
    check("显示命令", (r?.stdoutText ?? "").contains("$ cca login '工作(bob@example.com)'"))
    check("运行一次就删掉自己", !FileManager.default.fileExists(atPath: url.path))
    // 放了很久的清掉
    let old = dir.appendingPathComponent("cca-old.command")
    FileManager.default.createFile(atPath: old.path, contents: Data(), attributes: [.modificationDate: Date(timeIntervalSinceNow: -7200)])
    TerminalHandoff.cleanup(directory: dir)
    check("放了很久的交接文件清掉", !FileManager.default.fileExists(atPath: old.path))
    let noDisplay = EventLog.parseLine(Data(#"{"v":1,"ev":"terminal","code":"login","argv":["login","工作(bob@example.com)"],"message":""}"#.utf8))
    check("terminal 事件没给 display 时按参数拼（加引号）", noDisplay?.terminal.map { Handoff(event: $0).display } == "cca login '工作(bob@example.com)'")
    check("Handoff 的常用入口", Handoff.addAPI().arguments == ["add", "api"]
          && Handoff.routerSetup.arguments == ["router", "setup"] && Handoff.localSetup.arguments == ["local", "setup"]
          && Handoff.addAccount(prefix: ["codex"], product: "ChatGPT").arguments == ["codex", "add"]
          && Handoff.edit(prefix: ["codex"]).display == "cca codex edit")
}

func processTests() async {
    section("子进程")
    func sh(_ script: String, timeout: TimeInterval? = 10, env: [String: String] = ["PATH": "/usr/bin:/bin"],
            tweak: (inout ProcessSpec) -> Void = { _ in }) async -> ProcessResult? {
        var spec = ProcessSpec(executable: "/bin/sh", arguments: ["-c", script], environment: env, timeout: timeout)
        tweak(&spec)
        return try? await CcaProcess.run(spec)
    }
    let r1 = await sh("echo out; echo err >&2; exit 3")
    check("stdout / stderr 分开、退出码", r1?.stdoutText == "out\n" && r1?.stderrText == "err\n" && r1?.exitCode == 3)
    let r2 = await sh("cat; echo done")
    check("stdin 是 /dev/null", r2?.stdoutText == "done\n")
    let r3 = await sh("echo $$; ps -o pgid= -p $$")
    let parts = (r3?.stdoutText ?? "").split(whereSeparator: \.isWhitespace).map(String.init)
    check("自己一个进程组（会话）", parts.count == 2 && parts[0] == parts[1], "\(parts)")
    let r4 = await sh("if (exec 3</dev/tty) 2>/dev/null; then echo tty; else echo notty; fi")
    check("打不开 /dev/tty", r4?.stdoutText == "notty\n", r4?.stdoutText ?? "nil")
    let r5 = await sh("echo $FOO", env: ["PATH": "/usr/bin:/bin", "FOO": "bar 工作"])
    check("环境变量", r5?.stdoutText == "bar 工作\n")
    let start = Date()
    let r6 = await sh("sleep 30 & echo $!; wait", timeout: 0.5)
    let bgPid = Int32((r6?.stdoutText ?? "").trimmingCharacters(in: .whitespacesAndNewlines)) ?? 0
    try? await Task.sleep(nanoseconds: 300_000_000)
    check("超时：杀掉整个进程组", r6?.timedOut == true && Date().timeIntervalSince(start) < 5 && bgPid > 0 && kill(bgPid, 0) != 0,
          "timedOut=\(String(describing: r6?.timedOut)) bg=\(bgPid)")
    let s7 = Date()
    let r7 = await sh("trap '' TERM; while :; do sleep 0.1; done", timeout: 0.3) { $0.killGrace = 1 }
    check("不理 SIGTERM 的 1 秒后 SIGKILL", r7?.terminationSignal == SIGKILL && Date().timeIntervalSince(s7) < 4,
          "sig=\(String(describing: r7?.terminationSignal)) t=\(Date().timeIntervalSince(s7))")
    let s8 = Date()
    let task = Task { await sh("sleep 30", timeout: nil) }
    try? await Task.sleep(nanoseconds: 300_000_000)
    task.cancel()
    let r8 = await task.value
    check("取消：SIGTERM", r8?.cancelled == true && r8?.terminationSignal == SIGTERM && Date().timeIntervalSince(s8) < 3)
    let r9 = await sh("head -c 3000000 /dev/zero; echo x >&2")
    check("输出最多 2 MB", r9?.stdout.count == 2 * 1024 * 1024 && r9?.stdoutTruncated == true && r9?.stderrText == "x\n")
    let r10 = await sh(#"printf '\377\376ok'"#)
    check("宽松的 UTF-8", r10?.stdoutText.hasSuffix("ok") == true && r10?.stdoutText.contains("\u{FFFD}") == true)
    let s11 = Date()
    let r11 = await sh("(sleep 4) & echo hi")
    check("留下的后台进程不拖住", r11?.stdoutText == "hi\n" && Date().timeIntervalSince(s11) < 3, "\(Date().timeIntervalSince(s11))")
    do {
        _ = try await CcaProcess.run(ProcessSpec(executable: "/nonexistent/cca", arguments: [], environment: [:], timeout: 5))
        check("找不到程序报错", false)
    } catch let e as ProcessError {
        if case .spawnFailed = e { check("找不到程序报错", true) } else { check("找不到程序报错", false) }
    } catch {
        check("找不到程序报错", false)
    }

    // 输出接文件（app / web / router sync）：stdout、stderr 分开拿到；取消不杀；只留最后的部分
    let files = tmpRoot.appendingPathComponent("runs-unit")
    try? FileManager.default.createDirectory(at: files, withIntermediateDirectories: true)
    let marker = files.appendingPathComponent("done")
    var spec = ProcessSpec(executable: "/bin/sh", arguments: ["-c", "echo out; echo err >&2; sleep 1; touch \"$0\"; exit 4", marker.path],
                           environment: ["PATH": "/usr/bin:/bin"], timeout: 0.2)
    let out = files.appendingPathComponent("a.out").path, err = files.appendingPathComponent("a.err").path
    let s12 = Date()
    let fileTask = Task { try? await CcaProcess.runToFiles(spec, stdoutPath: out, stderrPath: err) }
    try? await Task.sleep(nanoseconds: 200_000_000)
    fileTask.cancel()
    let r12 = await fileTask.value
    check("输出接文件：stdout / stderr 分开、退出码", r12?.stdoutText == "out\n" && r12?.stderrText == "err\n" && r12?.exitCode == 4)
    check("输出接文件：取消、超时都不杀，跑完为止", r12?.cancelled == false && r12?.timedOut == false
          && FileManager.default.fileExists(atPath: marker.path) && Date().timeIntervalSince(s12) >= 1)
    check("输出接文件：读完删掉", !FileManager.default.fileExists(atPath: out) && !FileManager.default.fileExists(atPath: err))
    spec = ProcessSpec(executable: "/bin/sh", arguments: ["-c", "head -c 3000 /dev/zero | tr '\\0' a; printf END >&2"],
                       environment: ["PATH": "/usr/bin:/bin"], timeout: nil)
    spec.outputLimit = 1000
    let r13 = try? await CcaProcess.runToFiles(spec, stdoutPath: out, stderrPath: err)
    check("输出接文件：太长时留最后的部分", r13?.stdout.count == 1000 && r13?.stdoutTruncated == true && r13?.stderrText == "END")
}

/// 真的进程：假的 cca 脚本往 CCA_EVENTS 里写事件，ProcessTransport + CcaClient 整条路走一遍。
func processTransportTests() async {
    section("ProcessTransport")
    let dir = tmpRoot.appendingPathComponent("fake-cca")
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    let fake = dir.appendingPathComponent("cca")
    let script = #"""
    #!/bin/bash
    case "$*" in
      "_gui hello") printf '%s\n' '{"version":1,"kind":"hello","api":1,"cca_version":"9.9.9","installed":true,"tools":{"claude":{"enabled":true,"bin":null},"codex":{"enabled":false,"bin":null}}}' ;;
      env) env ;;
      "app k1")
        echo '{"v":1,"ev":"step","code":"sync"}' >> "$CCA_EVENTS"
        sleep 0.5
        if [ "${CCA_GRANT:-}" = "router_reload:OpenClash" ]; then
          echo '{"v":1,"ev":"step","code":"open"}' >> "$CCA_EVENTS"
          printf '%s' '{"v":1,"ev":"result","code":"opened","message":"已打开"}' >> "$CCA_EVENTS"
          exit 0
        fi
        echo '{"v":1,"ev":"consent","item":"router_reload","scope":"OpenClash","grant":"router_reload:OpenClash","title":"需要重载 OpenClash","question":"现在重载吗？","detail":"","severity":"warn","default":"yes"}' >> "$CCA_EVENTS"
        echo "没有重载" >&2
        exit 3 ;;
      "web k2")
        echo '{"v":1,"ev":"step","code":"open"}' >> "$CCA_EVENTS"
        echo "第一行"; echo "出错的说明" >&2
        sleep 1
        touch "$CCA_TEST_MARKER"
        echo '{"v":1,"ev":"fail","code":"open_failed","message":"没能打开"}' >> "$CCA_EVENTS"
        exit 5 ;;
      *) echo "unknown: $*" >&2; exit 2 ;;
    esac
    """#
    FileManager.default.createFile(atPath: fake.path, contents: Data(script.utf8), attributes: [.posixPermissions: 0o755])
    let eventsDir = tmpRoot.appendingPathComponent("events")
    let runsDir = tmpRoot.appendingPathComponent("runs")
    let marker = tmpRoot.appendingPathComponent("web-done")
    let base = ["PATH": "/usr/bin:/bin", "CCA_TTY": "/tmp/x", "CCA_YES": "1", "HOME": tmpRoot.path, "CCA_TEST_MARKER": marker.path]
    let t = ProcessTransport(ccaPath: fake.path, baseEnvironment: base, home: "/Users/me", eventsDirectory: eventsDir,
                             runsDirectory: runsDir)
    let c = CcaClient(transport: t)
    let h = try? await c.hello()
    check("hello 走真的进程", h?.ccaVersion == "9.9.9")
    let envResp = try? await t.send(CcaRequest(arguments: ["env"], timeout: 10, wantsEvents: true, grants: ["a:b"]), onEvent: nil)
    let lines = Set((envResp?.stdoutText ?? "").split(separator: "\n").map(String.init))
    check("子进程的环境", lines.contains("CCA_GUI=1") && lines.contains("NO_COLOR=1") && lines.contains("CCA_GRANT=a:b")
          && !lines.contains { $0.hasPrefix("CCA_TTY=") || $0.hasPrefix("CCA_YES=") }
          && lines.contains("PATH=/usr/bin:/bin:/Users/me/.local/bin:/opt/homebrew/bin:/usr/local/bin")
          && lines.contains { $0.hasPrefix("CCA_EVENTS=" + eventsDir.path) })
    let sink = EventSink()
    let p = ScriptedPrompter([true])
    let r = await c.execute(.app(.claude, key: "k1"), prompter: p, onEvent: { sink.add($0) })
    check("真的进程：同意后重跑成功", r.status == .success && r.runs == 2 && p.asked.first?.grant == "router_reload:OpenClash")
    check("运行中读到的事件（含最后一行没有换行的）", sink.events.compactMap(\.step).map(\.code) == [.sync, .sync, .open]
          && r.resultEvent?.message == "已打开", "\(sink.events)")
    let left = (try? FileManager.default.contentsOfDirectory(atPath: eventsDir.path)) ?? []
    check("事件文件用完删掉", left.isEmpty, "\(left)")

    // web 跑完为止：中途取消（App 退出）也不杀，输出从文件读回来
    let webTask = Task { await c.execute(.web(.claude, key: "k2"), prompter: nil) }
    try? await Task.sleep(nanoseconds: 300_000_000)
    webTask.cancel()
    let w = await webTask.value
    expectFailure("web 跑完为止：取消不杀，失败照常报", w.status) { $0.code == .openFailed && $0.exitCode == 5 && $0.stderrTail == "出错的说明" }
    check("web 跑完为止：cca 真的跑完了", FileManager.default.fileExists(atPath: marker.path))
    check("web 跑完为止：stdout 也读回来了", w.last?.stdoutText == "第一行\n")
    let runsLeft = (try? FileManager.default.contentsOfDirectory(atPath: runsDir.path)) ?? []
    check("运行输出文件用完删掉", runsLeft.isEmpty, "\(runsLeft)")
}

func eventTailTests() {
    section("事件文件")
    let url = tmpRoot.appendingPathComponent("tail.jsonl")
    FileManager.default.createFile(atPath: url.path, contents: Data())
    let tail = EventTail(url: url)
    check("空文件", tail.poll().isEmpty)
    let h = try! FileHandle(forWritingTo: url)
    h.write(Data(#"{"v":1,"ev":"step","code":"sync"}"#.utf8))
    check("没写完的行先不交", tail.poll().isEmpty)
    h.write(Data("\n".utf8))
    h.write(Data(#"{"v":1,"ev":"step","code":"open"}"#.utf8 + Data("\n".utf8)))
    let got = tail.poll()
    check("写完的行依次交出", got.compactMap(\.step).map(\.code) == [.sync, .open])
    check("交过的不再交", tail.poll().isEmpty)
    h.write(Data(#"{"v":1,"ev":"result","code":"opened"}"#.utf8))
    try? h.close()
    check("整份读最后一行没有换行也算", tail.all().count == 3 && tail.all().last?.result?.code == .opened)
}

/// 假的内核进程句柄：记下每次 terminate 的 wait。
final class FakeCoreHandle: CoreHandle, @unchecked Sendable {
    private let lock = NSLock()
    private var _terminated: [Bool] = []
    var terminated: [Bool] { lock.withLock { _terminated } }
    func terminate(wait: Bool) { lock.withLock { _terminated.append(wait) } }
}

@MainActor final class CoreStarts {
    var handles: [FakeCoreHandle] = []
    var exits: [@Sendable (Int32) -> Void] = []
    var failNext: Error?
}

@MainActor func coreHostTests() async {
    section("内置代理的内核（CoreHost）")
    let dir = tmpRoot.appendingPathComponent("core-host")
    let log = dir.appendingPathComponent("core.log").path
    let s = CoreStarts()
    let host = CoreHost(logPath: log, firstBackoff: 0.05, maxBackoff: 0.2, resetAfter: 0.3) { _, onExit in
        if let e = s.failNext {
            s.failNext = nil
            throw e
        }
        let h = FakeCoreHandle()
        s.handles.append(h)
        s.exits.append(onExit)
        return h
    }
    var changes: [CoreHost.Status] = []
    host.onChange = { changes.append($0) }
    func running() -> Bool { if case .running = host.status { return true }; return false }
    func waiting() -> (Date, Int32, String)? { if case .waiting(let at, let c, let d) = host.status { return (at, c, d) }; return nil }

    host.setWanted(true)
    check("要内核：启动一次，状态是运行中；日志目录建好（700）", s.handles.count == 1 && running()
          && (try? FileManager.default.attributesOfItem(atPath: dir.path))?[.posixPermissions] as? NSNumber == 0o700)
    host.setWanted(true)
    check("再说一遍要：不重复启动", s.handles.count == 1)

    try? "启动\n第二行\n内核退出的原因\n\n".write(toFile: log, atomically: true, encoding: .utf8)
    s.exits[0](3)
    _ = await waitUntil(2) { waiting() != nil || s.handles.count > 1 }
    if let w = waiting() {
        check("退出了：等着重启，带着退出码和日志的最后几行", w.1 == 3 && w.2.hasSuffix("内核退出的原因") && w.2.contains("第二行"), w.2)
    } else {
        check("退出了：等着重启，带着退出码和日志的最后几行", false, "\(host.status)")
    }
    check("退避之后自己重启", await waitUntil(2) { s.handles.count == 2 } && running())
    let t0 = Date()
    s.exits[1](1)
    _ = await waitUntil(2) { s.handles.count == 3 }
    check("马上又退出：退避翻倍（0.05 → 0.1 秒）", s.handles.count == 3 && Date().timeIntervalSince(t0) >= 0.09)
    s.exits[2](1)
    _ = await waitUntil(2) { waiting() != nil }
    check("退避最多到 maxBackoff", (waiting()?.0.timeIntervalSinceNow ?? 0) > 0.15 && (waiting()?.0.timeIntervalSinceNow ?? 1) <= 0.21)
    host.retryNow()
    check("现在重启：不等退避", s.handles.count == 4 && running())
    try? await Task.sleep(nanoseconds: 350_000_000)
    s.exits[3](0)
    _ = await waitUntil(2) { waiting() != nil }
    check("跑够 resetAfter 才退出的：退避从头算", (waiting()?.0.timeIntervalSinceNow ?? 1) <= 0.06)
    _ = await waitUntil(2) { s.handles.count == 5 }

    host.setWanted(false)
    check("不要了：停掉内核（不等），状态 off", s.handles[4].terminated == [false] && host.status == .off)
    s.exits[4](143)
    try? await Task.sleep(nanoseconds: 150_000_000)
    check("停掉的内核迟到的退出通知：不重启、状态不变", s.handles.count == 5 && host.status == .off)

    s.failNext = CcaError.launch("找不到 cca")
    host.setWanted(true)
    if let w = waiting() {
        check("起不来：等着重启（退出码 -1，说明原因）", w.1 == -1 && w.2.contains("找不到 cca"), w.2)
    } else {
        check("起不来：等着重启（退出码 -1，说明原因）", false, "\(host.status)")
    }
    check("起不来之后照样按退避重试", await waitUntil(2) { s.handles.count == 6 } && running())
    host.shutdown()
    check("App 退出：停掉并等它退出", s.handles[5].terminated == [true] && host.status == .off && !host.isWanted)
    check("状态变化都通知了", changes.first.map { if case .running = $0 { return true }; return false } == true && changes.last == .off)

    // 日志超过 1 MB：启动前清空
    try? Data(count: 1_100_000).write(to: URL(fileURLWithPath: log))
    let host2 = CoreHost(logPath: log) { _, _ in FakeCoreHandle() }
    host2.setWanted(true)
    check("日志超过 1 MB：启动前清空", !FileManager.default.fileExists(atPath: log))
    host2.shutdown()
}

func coreProcessTests() async {
    section("内置代理的内核（真的进程）")
    let dir = tmpRoot.appendingPathComponent("core-proc")
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    let fake = dir.appendingPathComponent("cca")
    let script = """
    #!/bin/bash
    [ "$*" = "_core run" ] || { echo "unknown: $*" >&2; exit 2; }
    echo "core started gui=$CCA_GUI"
    [ -f "\(dir.path)/exit7" ] && { echo "配置读不了" >&2; exit 7; }
    sh -c 'echo $$ > "\(dir.path)/child.pid"; exec sleep 30' &
    exec sleep 30
    """
    FileManager.default.createFile(atPath: fake.path, contents: Data(script.utf8), attributes: [.posixPermissions: 0o755])
    let t = ProcessTransport(ccaPath: fake.path, baseEnvironment: ["PATH": "/usr/bin:/bin"], home: dir.path)
    let log = dir.appendingPathComponent("core.log").path
    let exited = TriggerBox()
    do {
        let h = try t.startCore(logPath: log) { exited.set($0) }
        var ok = false
        for _ in 0..<50 {
            if ((try? String(contentsOfFile: log, encoding: .utf8)) ?? "").contains("core started gui=1") { ok = true; break }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        check("startCore 运行 cca _core run，输出追加进日志（带着 CCA_GUI=1）", ok)
        for _ in 0..<30 where !FileManager.default.fileExists(atPath: dir.appendingPathComponent("child.pid").path) {
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        let t0 = Date()
        h.terminate(wait: true)
        check("terminate(wait:) 停掉、等它退出，onExit 拿到被信号杀掉的退出码", exited.value == 128 + SIGTERM && Date().timeIntervalSince(t0) < 2.5,
              "\(String(describing: exited.value))")
        let child = Int32((try? String(contentsOfFile: dir.appendingPathComponent("child.pid").path, encoding: .utf8))?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? "") ?? 0
        var gone = false
        for _ in 0..<30 where !gone {
            gone = child > 0 && kill(child, 0) != 0
            if !gone { try? await Task.sleep(nanoseconds: 100_000_000) }
        }
        check("整个进程组都停了（cca 留下的子进程也是）", gone, "child=\(child)")
        let t1 = Date()
        h.terminate(wait: true)
        check("已经退出了：再停不发信号、不等", Date().timeIntervalSince(t1) < 0.2)
    } catch {
        check("startCore 运行 cca _core run", false, "\(error)")
    }
    FileManager.default.createFile(atPath: dir.appendingPathComponent("exit7").path, contents: Data())
    let exited2 = TriggerBox()
    _ = try? t.startCore(logPath: log) { exited2.set($0) }
    for _ in 0..<50 where exited2.value == nil { try? await Task.sleep(nanoseconds: 100_000_000) }
    let text = (try? String(contentsOfFile: log, encoding: .utf8)) ?? ""
    check("自己退出：退出码交给 onExit，stderr 也进日志（追加，不覆盖）", exited2.value == 7 && text.contains("配置读不了")
          && text.components(separatedBy: "core started").count == 3, text)
}

final class TriggerBox: @unchecked Sendable {
    private let lock = NSLock()
    private var _value: Int32?
    var value: Int32? { lock.withLock { _value } }
    func set(_ v: Int32) { lock.withLock { _value = v } }
}
