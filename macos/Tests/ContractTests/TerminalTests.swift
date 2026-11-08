import Foundation

// 在终端里打开账号：交接脚本（文件夹、留在 shell 里、和 App 抢 rename）、默认终端、启动目录的记忆、终端没运行起来时。

func sessionScriptTests() async {
    section("终端交接：账号的会话")
    let fm = FileManager.default
    let dir = tmpRoot.appendingPathComponent("handoff-session")
    let place = tmpRoot.appendingPathComponent("工作 目录'x")
    try? fm.createDirectory(at: place, withIntermediateDirectories: true)
    func tool(_ name: String, _ body: String) -> URL {
        let url = tmpRoot.appendingPathComponent(name)
        fm.createFile(atPath: url.path, contents: Data(body.utf8), attributes: [.posixPermissions: 0o755])
        return url
    }
    let cca = tool("session-cca", "#!/bin/sh\npwd\nfor a in \"$@\"; do printf '<%s>\\n' \"$a\"; done\nexit 3\n")
    let shell = tool("session-shell", "#!/bin/sh\necho \"SHELL-STARTED $*\"\n")
    func run(_ url: URL) async -> ProcessResult? {
        try? await CcaProcess.run(ProcessSpec(executable: "/bin/bash", arguments: [url.path],
                                              environment: ["PATH": "/usr/bin:/bin", "HOME": "/nonexistent", "SHELL": shell.path], timeout: 20))
    }

    let session = Handoff.session(prefix: ["codex"], key: "工作(bob@example.com)", name: "工作", directory: place.path)
    check("会话是 cca [codex] @账号", session.arguments == ["codex", "@工作(bob@example.com)"] && session.staysOpen && session.directory == place.path)
    guard let url = try? TerminalHandoff.write(session, ccaPath: cca.path, directory: dir) else {
        check("写会话的交接文件", false)
        return
    }
    let r = await run(url)
    let out = r?.stdoutText ?? ""
    check("在选的文件夹里运行", out.split(separator: "\n").contains { $0.hasSuffix("工作 目录'x") }, out)
    check("参数原样传给 cca", out.contains("<codex>\n<@工作(bob@example.com)>\n"), out)
    check("结束后留在用户的 shell 里（登录 shell）", out.contains("SHELL-STARTED -l"), out)
    check("说清楚退出码", out.contains("退出码 3"), out)
    check("运行一次就删掉自己", !fm.fileExists(atPath: url.path) && !fm.fileExists(atPath: url.path + ".run"))

    var gone = session
    gone.directory = tmpRoot.appendingPathComponent("没有这个文件夹").path
    if let u = try? TerminalHandoff.write(gone, ccaPath: cca.path, directory: dir) {
        let rr = await run(u)
        check("文件夹没了：说清楚，不在别的地方运行", rr?.exitCode == 1 && (rr?.stdoutText ?? "").contains("打不开这个文件夹") && !(rr?.stdoutText ?? "").contains("<codex>"),
              rr?.stdoutText ?? "")
    }

    // 普通的交接（登录、添加账号）照旧：不换文件夹，结束时等按键
    let plain = TerminalHandoff.script(Handoff(arguments: ["edit"], title: "编辑"), ccaPath: "/x/cca")
    check("普通交接不 cd、不进 shell", !plain.contains("cd --") && !plain.contains("exec \"${SHELL") && plain.contains("按任意键关闭"))

    // App 收回和脚本开始运行抢同一个 rename，只有一边成功
    if let u = try? TerminalHandoff.write(Handoff(arguments: ["edit"], title: "编辑"), ccaPath: cca.path, directory: dir) {
        check("收回还没运行的脚本", TerminalHandoff.reclaim(u) && !fm.fileExists(atPath: u.path))
        check("没了就收不回", !TerminalHandoff.reclaim(u))
        check("收回后不留文件", (try? fm.contentsOfDirectory(atPath: dir.path).filter { $0.hasSuffix(".cancelled") })?.isEmpty == true)
    }
    let locked = tmpRoot.appendingPathComponent("handoff-locked")
    let marker = tmpRoot.appendingPathComponent("ran-after-reclaim")
    let touching = tool("touch-cca", "#!/bin/sh\n/usr/bin/touch \(shellQuote(marker.path))\n")
    if let u = try? TerminalHandoff.write(Handoff(arguments: ["edit"], title: "编辑"), ccaPath: touching.path, directory: locked) {
        try? fm.setAttributes([.posixPermissions: 0o555], ofItemAtPath: locked.path)
        let rr = await run(u)
        try? fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: locked.path)
        check("脚本抢不到（App 已经收回）就什么也不做", rr?.exitCode == 0 && !fm.fileExists(atPath: marker.path))
    }
}

@MainActor func terminalModelTests() async {
    section("默认终端、启动目录")
    let fm = FileManager.default
    let bobRef = AccountRef(tool: .claude, key: "工作(bob@example.com)")
    let iTerm = TerminalApp.known.first { $0.id == "com.googlecode.iterm2" }!
    let places = ["项目甲", "项目乙"].map { tmpRoot.appendingPathComponent("places/" + $0) }
    for p in places { try? fm.createDirectory(at: p, withIntermediateDirectories: true) }
    let prefs = MemoryPrefs()
    let log = HookLog()
    log.installed = [.system, iTerm]
    let m = makeModel(fixtureTransport(), log: log, prefs: prefs)
    await m.start()

    check("认识的终端：系统自带的在最前面，每个 bundle id 只一个", TerminalApp.known.first == .system && Set(TerminalApp.known.map(\.id)).count == TerminalApp.known.count)
    let mainstream = ["com.googlecode.iterm2", "com.mitchellh.ghostty", "dev.warp.Warp-Stable", "com.github.wez.wezterm", "net.kovidgoyal.kitty", "org.alacritty",
                      "org.tabby", "com.raphaelamorim.rio", "co.zeit.hyper", "com.cmuxterm.app", "com.termius-dmg.mac", "dev.commandline.waveterm"]
    check("主流的第三方终端都认识", mainstream.allSatisfy { id in TerminalApp.known.contains { $0.id == id } })
    check("只有 Termius、Wave 是半自动", TerminalApp.known.filter { !$0.isAutomatic }.map(\.id) == ["com.termius-dmg.mac", "dev.commandline.waveterm"])
    check("带参数启动的都把脚本传过去", TerminalApp.known.allSatisfy { t in
        if case .arguments(let a) = t.launch { return a.contains(TerminalApp.scriptToken) }
        return true
    })
    let ghostty = TerminalApp.known.first { $0.id == "com.mitchellh.ghostty" }!
    check("识别出装了的终端，没选过时优先用第三方的", m.terminals == [.system, iTerm] && m.terminalChoice == iTerm)
    m.chooseTerminal(TerminalApp.system.id)
    check("也可以选回系统自带的", m.terminalChoice == .system)
    m.chooseTerminal(iTerm.id)
    check("选的终端记下来", m.terminalChoice == iTerm && prefs.string(forKey: AppModel.Prefs.terminalApp) == iTerm.id)
    m.chooseTerminal(ghostty.id)
    check("没装的选不了", !m.isInstalled(ghostty) && m.terminalChoice == iTerm && prefs.string(forKey: AppModel.Prefs.terminalApp) == iTerm.id)
    log.installed = [.system]
    m.refreshTerminals()
    check("选的终端卸载了就退回系统自带的", m.terminalChoice == .system)
    log.installed = [.system, iTerm]
    m.refreshTerminals()
    check("装回来又用它", m.terminalChoice == iTerm)

    check("第三方 API 账号也能在终端里打开", m.account(AccountRef(tool: .claude, key: "香港接口"))?.can.cli == true)
    check("主目录写成 ~", m.displayPath(tmpRoot.path) == "~" && m.displayPath(places[0].path) == "~/places/项目甲" && m.displayPath("/opt/x") == "/opt/x")

    m.openSession(bobRef, directory: places[1].path)
    m.openSession(bobRef, directory: places[0].path)
    m.openSession(bobRef, directory: places[1].path)
    check("文件夹：最近的在前、不重复", m.terminalDirectories == [places[1].path, places[0].path])
    check("演示模式不开终端", log.opened.isEmpty && (m.toast?.text ?? "").contains("演示模式"))

    let again = makeModel(fixtureTransport(), log: HookLog(), prefs: prefs)
    check("重新打开 App 还记得", again.terminalDirectories == [places[1].path, places[0].path] && again.terminalID == iTerm.id)
    again.shutdown()

    let many = (0..<12).map { i -> String in
        let p = tmpRoot.appendingPathComponent("places/n\(i)")
        try? fm.createDirectory(at: p, withIntermediateDirectories: true)
        return p.path
    }
    for p in many { m.openSession(bobRef, directory: p) }
    check("最多记 8 个", m.terminalDirectories.count == 8 && m.terminalDirectories.first == many.last)
    try? fm.removeItem(atPath: many.last!)
    check("不在了的文件夹不列出来", m.sessionDirectories.first == many[10] && m.sessionDirectories.count == 7)

    // 第一次（没有记过文件夹）：先选一个；取消就什么也不打开
    let firstLog = HookLog()
    let first = makeModel(fixtureTransport(), log: firstLog, prefs: MemoryPrefs())
    await first.start()
    first.chooseSessionDirectory(bobRef)   // 取消
    try? await Task.sleep(nanoseconds: 50_000_000)
    check("选文件夹时取消：不记、不打开", first.terminalDirectories.isEmpty && first.toast == nil)
    firstLog.pickedDirectory = places[0].path
    first.chooseSessionDirectory(bobRef)
    _ = await waitUntil { !first.terminalDirectories.isEmpty }
    check("选了就打开、记住", first.terminalDirectories == [places[0].path] && (first.toast?.text ?? "").contains("@工作(bob@example.com)"))
    first.shutdown()

    // 选的终端有没有运行交接脚本
    let handoffs = tmpRoot.appendingPathComponent("handoff-model")
    let openLog = HookLog()
    openLog.installed = [.system, iTerm]
    let w = makeModel(fixtureTransport(), log: openLog, prefs: MemoryPrefs())
    await w.start()
    w.handoffDirectory = handoffs
    w.terminalGrace = 0.05
    w.chooseTerminal(TerminalApp.system.id)
    w.launchInTerminal(.edit(prefix: []), ccaPath: "/usr/bin/true")
    check("选了系统自带的：用它", openLog.terminals == [TerminalApp.system.id])
    try? await Task.sleep(nanoseconds: 150_000_000)
    check("系统自带的不用等它", w.sheet == nil && openLog.opened.last.map { fm.fileExists(atPath: $0.path) } == true)

    w.chooseTerminal(iTerm.id)
    w.launchInTerminal(.edit(prefix: []), ccaPath: "/usr/bin/true")
    check("用选好的终端", openLog.terminals.last == iTerm.id)
    guard let notRun = openLog.opened.last else { return }
    _ = await waitUntil { w.sheet != nil }
    var fallback: Handoff?
    if case .handoff(let h)? = w.sheet { fallback = h }
    check("终端过了一会儿还没运行：收回脚本、问要不要改用系统自带的", fallback?.systemTerminal == true && (fallback?.title ?? "").contains("iTerm2") && !fm.fileExists(atPath: notRun.path))
    w.dismissSheet()
    if let fallback { w.launchInTerminal(fallback, ccaPath: "/usr/bin/true") }
    check("改用系统自带的", openLog.terminals.last == TerminalApp.system.id)

    w.launchInTerminal(.edit(prefix: []), ccaPath: "/usr/bin/true")
    if let running = openLog.opened.last { try? fm.moveItem(atPath: running.path, toPath: running.path + ".run") }   // 脚本开始运行时做的事
    try? await Task.sleep(nanoseconds: 150_000_000)
    check("已经开始运行就不打扰", w.sheet == nil)

    openLog.openSucceeds = false
    w.launchInTerminal(.edit(prefix: []), ccaPath: "/usr/bin/true")
    check("打不开时马上说", w.sheet != nil)
    w.dismissSheet()
    w.shutdown()

    // 半自动：打开它、命令进剪贴板，不写交接文件
    let termius = TerminalApp.known.first { $0.id == "com.termius-dmg.mac" }!
    let pasteLog = HookLog()
    pasteLog.installed = [.system, termius]
    let p = makeModel(fixtureTransport(), log: pasteLog, prefs: MemoryPrefs())
    await p.start()
    p.handoffDirectory = tmpRoot.appendingPathComponent("handoff-paste")
    p.terminalGrace = 0.05
    check("只装了半自动的终端：没选过时还是系统自带的", p.terminalChoice == .system)
    p.chooseTerminal(termius.id)
    check("半自动的终端可以选", p.terminalChoice == termius)
    p.launchInTerminal(.session(prefix: ["codex"], key: "工作(bob@example.com)", name: "工作", directory: "/tmp/工作 目录"), ccaPath: "/x/cca")
    check("半自动：打开它、命令进剪贴板（先进文件夹，cca 用绝对路径）",
          pasteLog.activated == [termius.id] && pasteLog.copied == ["cd '/tmp/工作 目录' && /x/cca codex '@工作(bob@example.com)'"], pasteLog.copied.joined(separator: "|"))
    check("半自动：提醒去粘贴", (p.toast?.text ?? "").contains("粘贴") && (p.toast?.text ?? "").contains("Termius"))
    try? await Task.sleep(nanoseconds: 150_000_000)
    check("半自动：不写交接文件、不等它运行", pasteLog.opened.isEmpty && !fm.fileExists(atPath: tmpRoot.appendingPathComponent("handoff-paste").path) && p.sheet == nil)
    pasteLog.openSucceeds = false
    p.launchInTerminal(.edit(prefix: []), ccaPath: "/x/cca")
    check("打不开也说清楚，命令还在剪贴板里", pasteLog.copied.last == "/x/cca edit" && (p.toast?.text ?? "").contains("打不开 Termius"))
    p.shutdown()
    m.shutdown()
}
