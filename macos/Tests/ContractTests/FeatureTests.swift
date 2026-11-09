import Foundation

// 2.9.0-beta.11 加的：Fable 的大块（份额的文字）、重置额度、检查更新、更新了什么。

func whatsNewTests() {
    section("更新了什么")
    let order = ["2.8.9", "2.9.0-beta.1", "2.9.0-beta.2", "2.9.0-beta.10", "2.9.0", "2.9.1", "2.10.0", "10.0.0"]
    let parsed = order.compactMap(AppVersion.init)
    check("版本号都认得", parsed.count == order.count)
    check("版本号的先后和 cca 的 ver_gt 一样：同号的测试版比正式版旧，beta 后面按数比（10 比 2 新），每一位按数比",
          zip(parsed, parsed.dropFirst()).allSatisfy { $0 < $1 })
    check("认不出的版本号", ["", "2.9", "2.9.0.1", "2.9.0-beta", "2.9.0-beta.x", "v2.9.0", "2.9.0-rc.1", "2.+9.0", "2.9.0-beta.-1",
                           "2.9.0-beta.1-beta.2"].allSatisfy { AppVersion($0) == nil })
    check("测试版、正式版", AppVersion("2.9.0-beta.3")?.isBeta == true && AppVersion("2.9.0")?.isBeta == false)

    let all = WhatsNew.all
    check("每一条的版本号都认得、不重复，最新的在前",
          all.allSatisfy { AppVersion($0.version) != nil } && Set(all.map(\.version)).count == all.count
          && zip(all, all.dropFirst()).allSatisfy { AppVersion($0.version)! > AppVersion($1.version)! })
    check("每一条都有标题和要点，要点都有图标和文字",
          all.allSatisfy { !$0.title.isEmpty && !$0.points.isEmpty && $0.points.allSatisfy { !$0.symbol.isEmpty && !$0.text.isEmpty } })
    let metaphors = ["办公室", "员工", "工位", "在忙", "休息", "回来"]
    check("界面上的文字不用办公室、员工这类比喻",
          all.allSatisfy { n in ([n.title] + n.points.map(\.text)).allSatisfy { t in !metaphors.contains { t.contains($0) } } })

    let notes = ["2.9.2", "2.9.1", "2.9.0", "2.9.0-beta.3"].map { ReleaseNote(version: $0, title: $0, points: []) }
    func launch(_ done: Bool, _ last: String?, _ cur: String) -> ([String], String?) {
        let r = WhatsNew.onLaunch(tutorialDone: done, lastSeen: last, current: cur, notes: notes)
        return (r.show.map(\.version), r.seen)
    }
    check("第一次装：只看新手教程，这个版本记作看过", launch(false, nil, "2.9.1") == ([], "2.9.1"))
    check("升级一个版本：给这个版本的，记下", launch(true, "2.9.0", "2.9.1") == (["2.9.1"], "2.9.1"))
    check("跳过了几个版本：都给（最新的在前），比现在新的不给", launch(true, "2.9.0-beta.3", "2.9.1") == (["2.9.1", "2.9.0"], "2.9.1"))
    check("测试版升到同号的正式版：正式版算新的", launch(true, "2.9.0-beta.3", "2.9.0") == (["2.9.0"], "2.9.0"))
    check("不知道上次看到哪个版本（从有这个功能之前的版本升上来）：只给这个版本的", launch(true, nil, "2.9.1") == (["2.9.1"], "2.9.1"))
    check("上次看的版本号认不出：当作没看过", launch(true, "x", "2.9.1") == (["2.9.1"], "2.9.1"))
    check("没升级：不给、不改", launch(true, "2.9.1", "2.9.1") == ([], nil))
    check("降级：不给、不改（再升回来也不重复给）", launch(true, "2.9.2", "2.9.1") == ([], nil))
    check("这个版本没写更新说明：不给，照样记下", launch(true, "2.9.2", "2.9.3") == ([], "2.9.3"))
    check("「更新说明」：不比现在新的都给（最新的在前）", WhatsNew.history(current: "2.9.1", notes: notes).map(\.version) == ["2.9.1", "2.9.0", "2.9.0-beta.3"])

    section("文字（这一版）")
    check("Fable 和本周总额度的关系", Texts.weeklyShare(allowance: 50, used: 3) == "最多用本周额度的 50%，已用 3%"
          && Texts.weeklyShare(allowance: 50, used: nil) == "最多用本周额度的 50%" && Texts.weeklyShare(allowance: nil, used: 3) == "单独计的每周额度")
    var cal = Calendar(identifier: .gregorian)
    cal.timeZone = TimeZone(identifier: "Asia/Shanghai")!
    let now = 1794042000.0   // 2026-11-07 17:00（上海）
    check("重置机会什么时候过期：一天以内写还有多久，今年以内不写年份",
          Texts.expiryText(epoch: now + 5 * 3600 + 120, now: now, calendar: cal) == "5 小时 2 分后过期"
          && Texts.expiryText(epoch: 1795251600, now: now, calendar: cal) == "11 月 21 日过期"
          && Texts.expiryText(epoch: 1798761600, now: now, calendar: cal) == "2027 年 1 月 1 日过期"
          && Texts.expiryText(epoch: now - 1, now: now, calendar: cal) == "已过期",
          Texts.expiryText(epoch: 1798761600, now: now, calendar: cal))
    func credits(_ count: Int?, expires: Double?) -> ResetCredits? {
        let obj: [String: Any] = ["availableCount": count ?? NSNull(), "next_expires_epoch": expires ?? NSNull()]
        return (try? JSONSerialization.data(withJSONObject: obj)).flatMap { try? JSONDecoder().decode(ResetCredits.self, from: $0) }
    }
    check("重置机会的到期日：几次时说最早一次，只有一次不说，不过期时没有",
          credits(2, expires: 1795251600).flatMap { Texts.resetExpiry($0, now: now, calendar: cal) } == "最早一次 11 月 21 日过期"
          && credits(1, expires: 1795251600).flatMap { Texts.resetExpiry($0, now: now, calendar: cal) } == "11 月 21 日过期"
          && credits(2, expires: nil).flatMap { Texts.resetExpiry($0, now: now, calendar: cal) } == nil)
    check("总览的一句：次数带到期日；0 次、不知道次数时没有",
          Texts.resetSummary(credits(2, expires: 1795251600), now: now, calendar: cal) == "还能重置额度 2 次，最早一次 11 月 21 日过期"
          && Texts.resetSummary(credits(1, expires: nil), now: now, calendar: cal) == "还能重置额度 1 次"
          && Texts.resetSummary(credits(0, expires: nil), now: now, calendar: cal) == nil
          && Texts.resetSummary(credits(nil, expires: 1795251600), now: now, calendar: cal) == nil
          && Texts.resetSummary(nil, now: now, calendar: cal) == nil)
}

@MainActor func featureModelTests() async {
    section("AppModel：重置额度、检查更新")
    let frankRef = AccountRef(tool: .codex, key: "frank@example.com")
    let yes = ScriptedPrompter([true])
    let m = makeModel(fixtureTransport(), prompter: yes, appVersion: "2.9.0")
    await m.start()
    let before = m.account(frankRef).flatMap { m.usage(for: $0) }
    check("frank 还能重置 2 次，最早的一次 11 月 21 日过期（cca 算好的）",
          before?.resetCredits?.availableCount == 2 && before?.resetCredits?.nextExpiresEpoch == 1795251600)
    let bob = m.account(AccountRef(tool: .claude, key: "工作(bob@example.com)")).flatMap { m.usage(for: $0) }
    let fable = bob?.modelScoped.first { $0.name == "Fable" }
    check("bob 的 Fable：份额（最多用一半、已用 17%）", fable?.window.utilization == 34 && fable?.allowancePercentOfWeekly == 50
          && fable?.usedPercentOfWeekly == 17)
    m.resetQuota(frankRef)
    check("重置中", m.activity(frankRef)?.kind == .reset)
    _ = await waitUntil { m.activity(frankRef) == nil }
    check("重置额度：先问（reset_credit，范围是账号目录名，默认不同意）", yes.asked.first?.item == .resetCredit
          && yes.asked.first?.grant == "reset_credit:frank-example-com-2b3c4d" && yes.asked.first?.defaultYes == false)
    let after = m.account(frankRef).flatMap { m.usage(for: $0) }
    check("同意后重置：提示用 cca 给的话，用量换成重置后的", m.toast?.text == "已重置 frank@example.com 的额度（现在 5h 0% · 周 0%）"
          && after?.resetCredits?.availableCount == 1 && after?.fiveHour?.utilization == 0, m.toast?.text ?? "nil")
    m.hooks.prompter = ScriptedPrompter([false])
    m.resetQuota(frankRef)
    _ = await waitUntil { m.activity(frankRef) == nil }
    check("不同意就不重置", m.toast?.text == "没有同意，没有做任何改动")

    check("还没检查过更新：没有新版本的提示", m.availableUpdate == nil)
    m.checkForUpdates(manual: true)
    _ = await waitUntil { !m.updateChecking }
    check("检查更新：有新版本 2.9.1（演示数据），说一声", m.updateInfo?.latest == "2.9.1" && m.availableVersion == "2.9.1"
          && (m.toast?.text ?? "").contains("有新版本 2.9.1"))
    m.dismissUpdate()
    check("以后再说：这个版本不再提示", m.availableUpdate == nil && m.updateDismissed == "2.9.1")
    m.checkForUpdates(manual: true)
    _ = await waitUntil { !m.updateChecking }
    check("手动检查：点过“以后再说”的版本也重新提示", m.availableVersion == "2.9.1" && m.updateDismissed == nil)
    m.installUpdate()
    check("演示模式不更新", !m.updating && (m.toast?.text ?? "").contains("演示模式"))
    m.shutdown()

    // 自动检查：看得见时、一天最多一次（查不了 3 小时后再查）；更新并重新打开：cca update 版本号 → gui update → 脱离 App 跑 --relaunch → 退出
    let stateNoCodex = String(decoding: fixture("state_nocodex.json") ?? Data(), as: UTF8.self)
    func updateDoc(_ latest: String) -> String {
        #"{"version":1,"kind":"update","cca_version":"9.9.9","channel":"stable","source":"releases","latest":"\#(latest)","available":true,"notes":"cca \#(latest)\n第二行"}"#
    }
    final class Box: @unchecked Sendable { var offline = false }
    func scripted(hello: String = tinyHello, state: String = stateNoCodex, latest: String = "9.9.10", failUpdate: Bool = false,
                  box: Box = Box(), delay: TimeInterval = 0) -> ScriptedTransport {
        let doc = updateDoc(latest)
        return ScriptedTransport(delay: delay) { req, _ in
            switch req.arguments {
            case ["_gui", "hello"]: return response(0, stdout: hello)
            case ["_gui", "state"]: return response(0, stdout: state)
            case ["update", "--check", "--json"]:
                return box.offline ? response(1, [.fail(FailEvent(code: .error, message: "拿不到最新版本号（网络不通，或 GitHub 暂时不可用）"))])
                    : response(0, stdout: doc)
            case ["update", "9.9.10"]: return failUpdate ? response(1, [.fail(FailEvent(code: .error, message: "9.9.10：签名校验没通过"))]) : response(0)
            case ["gui", "update"]: return response(0)
            default: return response(2, stderr: "unexpected \(req.arguments)")
            }
        }
    }
    func checks(_ t: ScriptedTransport) -> Int { t.requests.filter { $0.arguments == ["update", "--check", "--json"] }.count }

    let t = scripted()
    let log = HookLog()
    let mu = makeModel(t, log: log, appVersion: "9.9.9")
    await mu.start()
    try? await Task.sleep(nanoseconds: 200_000_000)
    check("看不见时不自动检查更新", checks(t) == 0)
    mu.setMainVisible(true)
    _ = await waitUntil { checks(t) == 1 && !mu.updateChecking }
    check("看得见时自动检查一次更新（带事件文件），有新版本就出现在顶上、不弹提示", checks(t) == 1 && mu.availableVersion == "9.9.10"
          && mu.toast == nil && t.requests.last { $0.arguments == ["update", "--check", "--json"] }?.wantsEvents == true)
    mu.setMainVisible(false)
    mu.setMainVisible(true)
    try? await Task.sleep(nanoseconds: 200_000_000)
    check("一天之内不再自动检查", checks(t) == 1)
    mu.checkForUpdatesIfDue(now: Date().addingTimeInterval(86400 + 60))
    _ = await waitUntil { checks(t) == 2 && !mu.updateChecking }
    check("过了一天再查", checks(t) == 2)
    mu.installUpdate()
    _ = await waitUntil { log.terminated == 1 }
    let steps = t.requests.map(\.arguments).filter { $0 == ["update", "9.9.10"] || $0 == ["gui", "update"] }
    let pid = String(ProcessInfo.processInfo.processIdentifier)
    check("更新并重新打开：先把 cca 更新到 9.9.10，再下载同版本的 App，再脱离 App 跑 --relaunch，然后退出",
          steps == [["update", "9.9.10"], ["gui", "update"]] && t.detached.map(\.arguments) == [["gui", "update", "--relaunch", pid]]
          && log.terminated == 1, "\(steps)")
    mu.shutdown()

    // 查不了：说 cca 的原因（fail 事件），3 小时后再自动查，不是每次打开窗口都查
    let box = Box()
    box.offline = true
    let to = scripted(box: box)
    let mo = makeModel(to, appVersion: "9.9.9")
    await mo.start()
    mo.setMainVisible(true)
    _ = await waitUntil { checks(to) == 1 && !mo.updateChecking }
    mo.setMainVisible(false)
    mo.setMainVisible(true)
    mo.checkForUpdatesIfDue(now: Date().addingTimeInterval(2 * 3600))
    try? await Task.sleep(nanoseconds: 200_000_000)
    check("自动检查查不了：不说，3 小时之内不再自动查", checks(to) == 1 && mo.toast == nil)
    mo.checkForUpdatesIfDue(now: Date().addingTimeInterval(3 * 3600 + 60))
    _ = await waitUntil { checks(to) == 2 && !mo.updateChecking }
    check("过了 3 小时再查", checks(to) == 2)
    mo.checkForUpdates(manual: true)
    _ = await waitUntil { checks(to) == 3 && !mo.updateChecking }
    check("手动检查查不了：说 cca 的原因（fail 事件的 message）", mo.toast?.text == "没能检查更新：拿不到最新版本号（网络不通，或 GitHub 暂时不可用）",
          mo.toast?.text ?? "nil")
    mo.shutdown()

    // 手动检查时正好有一次自动检查在跑：等它的结果，照样说一声
    let ts = scripted(delay: 0.4)
    let ms = makeModel(ts, appVersion: "9.9.9")
    await ms.start()
    ms.setMainVisible(true)
    _ = await waitUntil { ms.updateChecking }
    ms.checkForUpdates(manual: true)
    _ = await waitUntil { !ms.updateChecking }
    check("手动检查遇上正在跑的自动检查：不重复查，结果照样说一声", checks(ts) == 1 && (ms.toast?.text ?? "").contains("有新版本 9.9.10"),
          ms.toast?.text ?? "nil")
    ms.shutdown()

    // cca 已经比检查到的“最新版本”新（两次检查之间自己更新了）：不提示降级
    let tl = scripted(latest: "9.9.8")
    let ml = makeModel(tl, appVersion: "9.9.9")
    await ml.start()
    ml.checkForUpdates(manual: false)
    _ = await waitUntil { !ml.updateChecking && checks(tl) == 1 }
    check("检查到的版本不比现在的 cca 新：不提示（按版本号的先后，不是只看相不相同）", ml.updateInfo?.latest == "9.9.8" && ml.availableUpdate == nil)
    ml.shutdown()

    let tf = scripted(failUpdate: true)
    let logf = HookLog()
    let mf = makeModel(tf, log: logf, appVersion: "9.9.9")
    await mf.start()
    mf.checkForUpdates(manual: false)
    _ = await waitUntil { !mf.updateChecking }
    mf.installUpdate()
    _ = await waitUntil { !mf.updating }
    if case .failure(let info)? = mf.sheet {
        check("cca 没更新成：显示原因，不下载 App、不退出", info.title == "没能把 cca 更新到 9.9.10" && info.failure.message.contains("签名校验没通过")
              && !tf.requests.contains { $0.arguments == ["gui", "update"] } && logf.terminated == 0)
    } else {
        check("cca 没更新成：显示原因，不下载 App、不退出", false, "\(String(describing: mf.sheet))")
    }
    mf.shutdown()

    // 关掉了自动更新（cca update off）、开发模式：不自动检查（手动的照查）；开发模式下也不提示安装
    var off = jsonObject(Data(stateNoCodex.utf8)) as? [String: Any] ?? [:]
    var cfg = off["config"] as? [String: Any] ?? [:]
    cfg["update"] = "off"
    off["config"] = cfg
    let offState = String(decoding: (try? JSONSerialization.data(withJSONObject: off)) ?? Data(), as: UTF8.self)
    let devHello = tinyHello.replacingOccurrences(of: #""installed":true"#, with: #""installed":true,"dev_mode":true"#)
    for (label, tt, dev) in [("关掉了自动更新", scripted(state: offState), false), ("开发模式", scripted(hello: devHello), true)] {
        let mm = makeModel(tt, appVersion: "9.9.9")
        await mm.start()
        mm.setMainVisible(true)
        try? await Task.sleep(nanoseconds: 300_000_000)
        let n = checks(tt)
        mm.checkForUpdates(manual: true)
        _ = await waitUntil { !mm.updateChecking && checks(tt) == 1 }
        check("\(label)：不自动检查更新，手动的照查", n == 0 && checks(tt) == 1)
        check("\(label)：有新版本时\(dev ? "只说用 git pull，不提示安装" : "照常提示")",
              dev ? mm.availableUpdate == nil && (mm.toast?.text ?? "").contains("git pull") : mm.availableVersion == "9.9.10")
        mm.shutdown()
    }
}
