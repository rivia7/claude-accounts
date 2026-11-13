import Foundation

// API key：掩码、按需经管道读明文、机密的剪贴板；明文不进打印、提示、读屏。

/// 手写一个登录着的 Claude 官方账号；extra 里的值覆盖默认
private func account(_ extra: [String: Any] = [:]) -> Account {
    var d: [String: Any] = ["key": "a@example.com", "kind": "oauth", "login": "logged_in", "plan": "Max", "can": ["check": true]]
    for (k, v) in extra { d[k] = v }
    return Account.sample(d, tool: .claude)
}

/// 假的剪贴板：记着内容、是不是标成了机密
@MainActor private final class FakeBoard: SecretPasteboard {
    var changeCount = 0
    var contents: String?
    var concealed = false

    func writeSecret(_ s: String) -> Int {
        changeCount += 1
        contents = s
        concealed = true
        return changeCount
    }

    func clear() {
        changeCount += 1
        contents = nil
        concealed = false
    }

    /// 用户自己复制了别的东西
    func userCopies(_ s: String) {
        changeCount += 1
        contents = s
        concealed = false
    }
}

@MainActor
func apiKeyTests() async {
    section("API key")
    let saved = account(["api_key": ["saved": true, "hint": "sk-ant-…x7Qa", "source": "keychain"], "can": ["check": true, "api_key": true]])
    check("api_key：解码", saved.apiKey == APIKeyInfo(saved: true, hint: "sk-ant-…x7Qa", source: .keychain) && saved.can.apiKey)
    check("旧版 cca 没有 api_key、can.api_key：没有入口", account().apiKey == nil && !account().can.apiKey)
    check("api_key：类型不对的字段当作没存", account(["api_key": ["saved": "yes", "hint": 3]]).apiKey == APIKeyInfo(saved: false, hint: "", source: .none))
    check("旧版 cca 的 api_key 没有同步的字段：当作不同步", saved.apiKey?.shared == false && saved.apiKey?.fromDevice == nil
          && saved.apiKey?.savedEpoch == nil && saved.apiKey?.syncError == nil)
    let synced = account(["api_key": ["saved": true, "hint": "sk-ant-…x7Qa", "source": "keychain", "shared": true, "saved_epoch": 1794020000.5,
                                      "from_device": "dan-air", "sync_error": NSNull()]])
    check("api_key：同步来的（来源、记下的时间）", synced.apiKey == APIKeyInfo(saved: true, hint: "sk-ant-…x7Qa", source: .keychain, shared: true,
                                                                       savedEpoch: 1794020000.5, fromDevice: "dan-air"))
    check("api_key：空的来源、原因当作没有", account(["api_key": ["saved": true, "hint": "", "source": "keychain", "shared": true, "from_device": "",
                                                       "sync_error": ""]]).apiKey == APIKeyInfo(saved: true, hint: "", source: .keychain, shared: true))

    // 账号页那一行下面的一句（只有 cca 说会同步时才有）
    var cal = Calendar(identifier: .gregorian)
    cal.timeZone = TimeZone(identifier: "Asia/Shanghai")!
    let now = 1794042000.0   // 2026-11-07 17:00（上海）
    let mine = APIKeyInfo(saved: true, hint: "h", source: .keychain, shared: true, savedEpoch: now - 3600)
    check("同步：这台 Mac 记下的", Texts.apiKeyShare(mine, now: now, calendar: cal) == "这台 Mac 16:00 记下，登录了这个账号的其他设备也能看到（加密同步）。",
          Texts.apiKeyShare(mine, now: now, calendar: cal) ?? "nil")
    var from = mine
    from.fromDevice = "dan-air"
    from.savedEpoch = now - 86400
    check("同步：别的设备记下、同步来的", Texts.apiKeyShare(from, now: now, calendar: cal) == "dan-air 昨天 17:00 记下，已加密同步到这台 Mac。",
          Texts.apiKeyShare(from, now: now, calendar: cal) ?? "nil")
    from.savedEpoch = nil
    check("同步：不知道什么时候记下的就不写时间", Texts.apiKeyShare(from, now: now, calendar: cal) == "dan-air 记下，已加密同步到这台 Mac。")
    check("同步：还没记下", Texts.apiKeyShare(APIKeyInfo(saved: false, hint: "", source: .none, shared: true), now: now)?.hasPrefix("记下后，登录了这个账号的其他设备也能看到") == true)
    var failing = from
    failing.syncError = "设备统计的服务器还不收 API key（维护者要更新服务器：./policy.sh presence deploy）"
    check("同步：没同步成时照 cca 的原因说", Texts.apiKeyShare(failing, now: now) == "没能和其他设备同步：设备统计的服务器还不收 API key（维护者要更新服务器：./policy.sh presence deploy）")
    check("不同步的账号：不说", Texts.apiKeyShare(APIKeyInfo(saved: true, hint: "h", source: .keychain, fromDevice: "dan-air"), now: now) == nil)
    check("「记录…」的说明：会同步的说清楚", Texts.apiKeyRecordHelp(isAPI: false, shared: true).contains("加密同步给登录了这个账号的其他设备")
          && Texts.apiKeyRecordHelp(isAPI: false, shared: false).contains("只存进这台 Mac")
          && Texts.apiKeyRecordHelp(isAPI: true, shared: true) == "输入这个账号的 key，存进这台 Mac 的钥匙串")

    // 明文：只收 cca 打出来的一串可见 ASCII，打印、调试输出、dump 里都只是「<密文>」
    let key = "sk-test-" + String(repeating: "x", count: 30)
    let v = APIKeyValue(cca: Data(key.utf8))
    check("cca 给的 key：原样收下", v?.text == key)
    check("不是 key 的不收：结尾换行、空格、太短、非 ASCII", APIKeyValue(cca: Data((key + "\n").utf8)) == nil
          && APIKeyValue(cca: Data("abc def ghi".utf8)) == nil && APIKeyValue(cca: Data("short".utf8)) == nil
          && APIKeyValue(cca: Data("密钥密钥密钥密钥".utf8)) == nil)
    if let v {
        var dumped = ""
        dump(v, to: &dumped)
        check("明文不进打印、调试输出、dump", "\(v)" == "<密文>" && String(reflecting: v) == "<密文>" && !dumped.contains(key) && !"\([v])".contains(key))
    }

    // 点开看：读一次、显示，过一会儿自己收起来；晚到的结果不要
    let r = KeyReveal(autoHide: .milliseconds(200))
    r.toggle { v }
    check("点眼睛：先是正在读", r.phase == .loading)
    _ = await waitUntil { r.phase == .shown }
    check("读到了：显示明文", r.phase == .shown && r.secret?.text == key)
    var rd = ""
    dump(r, to: &rd)
    check("KeyReveal 的描述、dump 里没有明文", !"\(r)".contains(key) && !rd.contains(key))
    _ = await waitUntil(2) { r.phase == .hidden }
    check("过一会儿自己收起来，明文丢掉", r.phase == .hidden && r.secret == nil)
    r.toggle { v }
    _ = await waitUntil { r.phase == .shown }
    r.toggle { v }
    check("再点一下：收起来", r.phase == .hidden && r.secret == nil)
    let slow = KeyReveal()
    slow.toggle {
        try? await Task.sleep(for: .milliseconds(150))
        return v
    }
    slow.hide()
    try? await Task.sleep(for: .milliseconds(300))
    check("收起来以后晚到的结果不显示", slow.phase == .hidden && slow.secret == nil)
    let failed = KeyReveal()
    failed.toggle { nil }
    _ = await waitUntil { failed.phase == .hidden }
    check("读不到：收起来（原因 AppModel 已经提示过）", failed.phase == .hidden && failed.secret == nil)

    // 剪贴板：标成机密；过一会儿剪贴板还是它就清掉，期间复制了别的就不动；退出前也清
    let board = FakeBoard()
    let clip = SecretClipboard(board: board, delay: .milliseconds(150))
    clip.copy(key)
    check("复制：写进剪贴板、标成机密", board.contents == key && board.concealed)
    _ = await waitUntil(2) { board.contents == nil }
    check("过一会儿剪贴板还是它：清掉", board.contents == nil)
    clip.copy(key)
    board.userCopies("别的东西")
    try? await Task.sleep(for: .milliseconds(300))
    check("期间复制了别的：不动", board.contents == "别的东西")
    clip.copy(key)
    clip.flush()
    check("退出 App 前：还是它就清掉", board.contents == nil)

    // 命令：输出只经内存里的管道，不写文件（runsToCompletion 的 stdout 写进 runs 目录的文件）
    let show = CcaCommand.keyShow(.claude, key: "k")
    check("key --show：参数", show.arguments == ["key", "k", "--show"]
          && CcaCommand.keyShow(.codex, key: "k").arguments == ["codex", "key", "k", "--show"])
    check("key --show：不写文件、用户在等、40 秒", !show.runsToCompletion && !show.isBackground && show.timeout == 40)
    check("key --forget：参数", CcaCommand.keyForget(.codex, key: "k").arguments == ["codex", "key", "k", "--forget"])
    let record = Handoff.apiKey(prefix: [], key: "k", name: "工作", record: true)
    check("记下 / 更换：App 里的提问会话（密文框）", record.inApp && record.arguments == ["key", "k"] && record.title == "记录 工作 的 API key"
          && Handoff.apiKey(prefix: [], key: "k", name: "工作").title == "更换 工作 的 API key")

    // AppModel：复制只经 copySecret，不进提示、读屏和普通的复制；读不到时弹框说 cca 给的原因
    guard let hello = fixture("hello.json").map({ String(decoding: $0, as: UTF8.self) }),
          let state = fixture("state.json").map({ String(decoding: $0, as: UTF8.self) }) else {
        check("读演示数据", false)
        return
    }
    let bobKey = "工作(bob@example.com)", carol = "carol@example.com"
    let log = HookLog()
    let t = ScriptedTransport { request, _ in
        if request.arguments == ["_gui", "hello"] { return response(0, stdout: hello) }
        if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
        if request.arguments == ["key", bobKey, "--show"] { return response(0, stdout: key) }
        if request.arguments == ["key", carol, "--show"] {
            return response(1, [.fail(FailEvent(code: .keyMissing, message: "carol@example.com 还没有存 API key"))], stderr: "还没有存")
        }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    let m = makeModel(t, log: log, prefs: MemoryPrefs())
    await m.start()
    guard let bob = m.account(AccountRef(tool: .claude, key: bobKey)), let c = m.account(AccountRef(tool: .claude, key: carol)) else {
        check("演示数据里有 bob、carol", false)
        m.shutdown()
        return
    }
    check("演示数据：bob 存了 key，只有掩码（从 dan-air 同步来的）；carol 没存",
          bob.apiKey == APIKeyInfo(saved: true, hint: "sk-ant-…x7Qa", source: .keychain, shared: true, savedEpoch: 1793955600, fromDevice: "dan-air")
          && c.apiKey?.saved == false && bob.can.apiKey)
    m.copyAPIKey(bob)
    _ = await waitUntil { m.toast != nil }
    check("复制 API key：只经 copySecret；提示、读屏、普通的复制里都没有 key", log.copiedSecrets == [key] && log.copied.isEmpty
          && m.toast?.text.contains(key) == false && !log.announced.contains { $0.contains(key) }, m.toast?.text ?? "")
    let req = t.requests.last { $0.arguments.contains("--show") }
    check("读 key：不是后台任务、不写文件、带事件文件", req?.background == false && req?.runsToCompletion == false && req?.wantsEvents == true)
    let read = await m.readAPIKey(c)
    _ = await waitUntil { m.sheet != nil }
    if case .failure(let info)? = m.sheet {
        check("读不到：弹框说 cca 给的原因", read == nil && info.failure.code == .keyMissing && info.failure.message.contains("还没有存"))
    } else {
        check("读不到：弹框说 cca 给的原因", false)
    }
    m.shutdown()
}
