import Foundation

// MARK: - 以下只在 --full（手写的 Tests/Fixtures）里跑

func loadFixture<T: CcaDocument>(_ type: T.Type, _ name: String) -> T? {
    guard let d = fixture(name) else {
        check("有 \(name)", false)
        return nil
    }
    return try? DocumentDecoder.decode(T.self, from: d)
}

func fixtureAssertions() {
    section("手写数据")
    let bobRef = AccountRef(tool: .claude, key: "工作(bob@example.com)")
    if let h = loadFixture(HelloDoc.self, "hello.json") {
        check("hello：启用了 Codex、已安装", h.codexEnabled && h.installed && h.api == 1 && h.ccaVersion == "2.9.0")
        check("hello：gui", h.gui?.appVersion == "2.9.0" && h.gui?.stagedVersion == nil)
    }
    if let h = loadFixture(HelloDoc.self, "hello_nocodex_notinstalled.json") {
        check("hello_nocodex_notinstalled：没安装、没启用 Codex", !h.installed && !h.codexEnabled && h.tools[.claude]?.bin == nil)
        check("hello_nocodex_notinstalled：gui.app_version 为 null", h.gui?.appVersion == nil)
    }
    if let s = loadFixture(StateDoc.self, "state.json") {
        let c = s.tool(.claude), x = s.tool(.codex)
        check("state：Claude 3 个官方账号 + 2 个 API 账号", c?.accounts.filter { !$0.isAPI }.count == 3 && c?.accounts.filter(\.isAPI).count == 2)
        let bob = s.account(bobRef)
        check("state：bob 被锁定、登录着、上次用的、App 开着", bob?.pinned == true && bob?.pinnedNode == "US-B" && bob?.login == .loggedIn
              && bob?.isLast == true && bob?.appRunning == true && bob?.app?.pid == 4242)
        check("state：bob 的缓存用量", bob?.usage?.fiveHour?.utilization == 92 && bob?.usage?.source == .cache
              && bob?.usage?.sevenDaySonnet?.utilization == 60 && bob?.usage?.modelScoped.first?.name == "Sonnet")
        check("state：bob 的 DNS 泄露", bob?.health?.leak?.dns == .leak && bob?.health?.leak?.webrtc == .ok && bob?.health?.leak?.hasLeak == true
              && bob?.health?.trust == 97 && bob?.health?.tags == ["住宅IP"])
        let carol = s.account(AccountRef(tool: .claude, key: "carol@example.com"))
        check("state：carol 没登录、查不了用量", carol?.login == .loggedOut && carol?.can.usage == false
              && carol?.canWhy["usage"] == .notLoggedIn && carol?.usage == nil)
        let dave = s.account(AccountRef(tool: .claude, key: "测试(dave@example.com)"))
        check("state：dave 登错号", dave?.login == .emailMismatch && dave?.loginEmail == "eve@example.com")
        let direct = s.account(AccountRef(tool: .claude, key: "直连接口"))
        check("state：直连的 API 账号", direct?.isAPI == true && direct?.port == nil && direct?.api?.host == "api.example.com"
              && direct?.health?.kind == .api && direct?.health?.port == nil && direct?.health?.route == .direct
              && direct?.loginEmail == "")
        let hk = s.account(AccountRef(tool: .claude, key: "香港接口"))
        check("state：走节点的 API 账号", hk?.isAPI == true && hk?.port == 7904 && hk?.node == "HK-A" && hk?.badge == nil)
        check("state：Codex 启用、1 个账号", x?.enabled == true && x?.accounts.count == 1 && x?.cliPrefix == ["codex"])
        let frank = x?.accounts.first
        check("state：frank 的 credits", frank?.usage?.credits?.balance == "12.5" && frank?.usage?.credits?.hasCredits == true
              && frank?.usage?.resetCredits?.availableCount == 2 && frank?.usage?.stale == true)
        check("state：frank 换了节点的旧检测结果", frank?.health?.applies == false && frank?.health?.stale == true
              && frank?.health?.result == .lowPurity && frank?.health?.leak == nil)
        check("state：8 个槽位、3 个空的", s.routing?.slots.count == 8 && s.routing?.slots.filter(\.isEmpty).count == 3)
        check("state：路由器待同步", s.routing?.sync == .pending && s.routing?.localSync == .ok)
        check("state：notices", s.notices.count == 2 && s.notices[0].code == .policyPinned && s.notices[0].oldNode == "US-A"
              && s.notices[1].code == .nodesFilled && s.notices[1].tool == nil && s.notices[1].node == "HK-A")
        check("state：config", s.config?.routerConfigured == true && s.config?.localConfigured == true && s.config?.minTrust == 50
              && s.config?.local?.client == .clashVerge && s.config?.policy?.repo == "example/cca-policy")
        check("state：本机 Clash 的叫法", s.routing?.slot(1)?.localNode == "Verge-B")
    }
    if let s = loadFixture(StateDoc.self, "state_nocodex.json") {
        let c = s.tool(.claude)
        check("state_nocodex：Codex 没启用", s.tool(.codex)?.enabled == false && s.tool(.codex)?.accounts.isEmpty == true)
        check("state_nocodex：账号表校验不过", c?.tableInvalid.count == 1 && c?.tableInvalid.first?.message.isEmpty == false)
        check("state_nocodex：settings_problems", c?.settingsProblems == ["settings.json:ANTHROPIC_BASE_URL"] && c?.chrome == false)
        let alice = c?.accounts.first
        check("state_nocodex：alice 没缓存、没装 App 和 Chrome", alice?.usage == nil && alice?.canWhy["app"] == .appMissing
              && alice?.canWhy["web"] == .noChrome && alice?.plan == "Pro 200")
        check("state_nocodex：policy_alias", s.notices.contains { $0.code == .policyAlias && $0.oldKey == "alice@example.com" })
    }
    if let u = loadFixture(UsageDoc.self, "usage_claude.json") {
        let bob = u.accounts.first { $0.key == bobRef.key }
        check("usage_claude：bob 实时", bob?.source == .live && bob?.fiveHour?.utilization == 92 && bob?.errorCode == nil)
        check("usage_claude：带小数的 resets_epoch", abs((bob?.fiveHour?.resetsEpoch ?? 0) - 1794050010.12) < 0.001)
        check("usage_claude：额外用量", bob?.extraUsage?.isEnabled == true && bob?.extraUsage?.utilization == 12.5)
        let carol = u.accounts.first { $0.key == "carol@example.com" }
        check("usage_claude：carol 用缓存、带出错原因", carol?.source == .cache && carol?.errorCode == .login && carol?.stale == true
              && carol?.fiveHour?.reset == true)
        let dave = u.accounts.first { $0.key == "测试(dave@example.com)" }
        check("usage_claude：dave 没数据、登错号", dave?.source == UsageSource.none && dave?.errorCode == .mismatch
              && dave?.errorDetail == "eve@example.com" && dave?.fetchedEpoch == nil)
        check("usage_claude：跳过的 API 账号", u.skippedAPIAccounts == ["直连接口", "香港接口"] && u.tool == nil)
        check("usage_claude：generated_epoch", u.generatedEpoch == 1794042000)
    }
    if let u = loadFixture(UsageDoc.self, "usage_codex.json") {
        let f = u.accounts.first
        check("usage_codex：tool、credits", u.tool == .codex && f?.credits?.balance == "12.5" && f?.credits?.unlimited == false)
        check("usage_codex：reset_credits", f?.resetCredits?.availableCount == 2)
        check("usage_codex：整数的 resets_at", f?.fiveHour?.resetsEpoch == 1794049200 && f?.sevenDay?.resetsEpoch == 1794470400)
    }
    if let l = loadFixture(LiveDoc.self, "live_router.json") {
        let states = Set(l.slots.map(\.state))
        check("live_router：ok、drift、closed、unknown、empty 都有", states == [.ok, .drift, .closed, .unknown, .empty])
        check("live_router：路由器", l.route == .router && l.router?.backend == .openClash && l.router?.hook == true
              && l.router?.cron == true && l.atHome == true && l.hasDrift)
        check("live_router：drift 的槽位", l.slot(2)?.expected == "US-C" && l.slot(2)?.selected == "US-A")
    }
    if let l = loadFixture(LiveDoc.self, "live_router.json") {
        check("live_router：proxy_host", l.proxyHost == "192.0.2.10")
    }
    if let s = loadFixture(StateDoc.self, "state_account_unreadable.json") {
        check("state_account_unreadable：读不了的账号当作没登录", s.errors.first?.code == .accountUnreadable && s.errors.first?.tool == .claude
              && s.tool(.claude)?.accounts.first?.login == .loggedOut)
    }
    if let l = loadFixture(LiveDoc.self, "live_router_drift.json") {
        check("live_router_drift：有 drift", l.hasDrift && l.summary?.drift == 2)
    }
    if let l = loadFixture(LiveDoc.self, "live_local.json") {
        check("live_local：本机 Clash", l.route == .local && l.local?.running == true && l.router?.reachable == false
              && l.router?.error == nil && l.slot(1)?.expected == "Verge-B" && l.proxyHost == "127.0.0.1")
    }
    if let l = loadFixture(LiveDoc.self, "live_unreachable.json") {
        check("live_unreachable：已分配的槽位都 closed", l.slots.filter { $0.tool != nil }.allSatisfy { $0.state == .closed }
              && l.router?.reachable == false && l.router?.error?.code == .unreachable && l.summary?.closed == 5
              && l.proxyHost == "192.0.2.10")
    }
    if let c = loadFixture(CheckDoc.self, "check.json") {
        check("check：3 个账号、共用出口", c.accounts.count == 3 && c.sharedExitIPs == ["203.0.113.7"] && c.route == .router)
        check("check：直连的 API 账号没有端口", c.accounts.last?.port == nil && c.accounts.last?.node == nil
              && c.accounts.last?.health?.result == .apiUnreachable && c.accounts.last?.health?.route == .direct)
        let r = c.accounts[1].health
        check("check：受限地区（纯净度按 0 算）", r?.result == .restricted && r?.restrictedRegion == "中国大陆" && r?.trust == 0
              && r?.trustLabel == "可疑" && r?.leak == nil)
        check("check：check 的 health 不带 age_seconds / stale / applies", c.accounts.allSatisfy { $0.health?.ageSeconds == nil })
    }
    let consent = fixture("events_consent.jsonl").map(EventLog.parse) ?? []
    check("events_consent：低纯度要同意、默认不同意", consent.compactMap(\.consent).first.map {
        $0.item == .lowPurity && !$0.defaultYes && $0.grant == "low_purity:203.0.113.20" && $0.severity == .warn
    } ?? false)
    let open = fixture("events_open.jsonl").map(EventLog.parse) ?? []
    check("events_open：四步和结果", open.compactMap(\.step).map(\.code) == [.sync, .preflight, .prepare, .open]
          && open.last?.result?.code == .opened && open.last?.result?.port == 7901)
    let fail = fixture("events_fail.jsonl").map(EventLog.parse) ?? []
    check("events_fail：最后一条是 port_closed", fail.compactMap(\.fail).last?.code == .portClosed && fail.compactMap(\.fail).count == 2)
    let term = fixture("events_terminal.jsonl").map(EventLog.parse) ?? []
    check("events_terminal：在终端里登录", term.compactMap(\.terminal).first.map {
        $0.code == .login && $0.argv == ["login", "carol@example.com"] && $0.display == "cca login carol@example.com"
    } ?? false)
    let sync = fixture("events_sync.jsonl").map(EventLog.parse) ?? []
    check("events_sync：重载 OpenClash 默认同意", sync.compactMap(\.consent).first.map {
        $0.item == .routerReload && $0.defaultYes && $0.grant == "router_reload:OpenClash"
    } ?? false)

    // 时间只用 *_epoch：旧字段 resets_at / fetched_at 不解析（*_epoch 为 null 就不显示）
    let w1 = try? JSONDecoder().decode(UsageWindow.self, from: Data(#"{"utilization": 1, "resets_at": 1794049200}"#.utf8))
    check("只用 resets_epoch：缺它时没有重置时间", w1 != nil && w1?.resetsEpoch == nil)
    let w2 = try? JSONDecoder().decode(UsageWindow.self, from: Data(#"{"utilization": 1, "resets_at": "2026-11-07T11:13:30+00:00", "resets_epoch": null}"#.utf8))
    check("只用 resets_epoch：为 null 时没有重置时间", w2 != nil && w2?.resetsEpoch == nil && w2?.isReset(now: 1e10) == false)
    let e1 = try? JSONDecoder().decode(UsageEntry.self, from: Data(#"{"key": "k", "source": "cache", "fetched_at": "2026-11-07T08:56:00Z"}"#.utf8))
    check("只用 fetched_epoch", e1 != nil && e1?.fetchedEpoch == nil)
    // 文档种类不对
    let wrongKind = try? DocumentDecoder.decode(HelloDoc.self, from: fixture("state.json") ?? Data())
    check("kind 不对时不收", wrongKind == nil)
    check("stdout 前面混了别的行也能解码", (try? DocumentDecoder.decode(HelloDoc.self, from: Data(("提示\n" + String(decoding: fixture("hello.json") ?? Data(), as: UTF8.self).replacingOccurrences(of: "\n", with: " ")).utf8))) != nil)
    check("空输出报错", (try? DocumentDecoder.decode(HelloDoc.self, from: Data("  \n".utf8))) == nil)
    // 缺 key 的账号丢掉，别的照样
    if let d = fixture("state.json"), var obj = jsonObject(d) as? [String: Any], var tools = obj["tools"] as? [[String: Any]],
       var accounts = tools[0]["accounts"] as? [[String: Any]] {
        accounts[1].removeValue(forKey: "key")
        tools[0]["accounts"] = accounts
        obj["tools"] = tools
        let s = (try? JSONSerialization.data(withJSONObject: obj)).flatMap { try? DocumentDecoder.decode(StateDoc.self, from: $0) }
        check("缺 key 的账号丢掉、别的照样", s?.tool(.claude)?.accounts.count == 4)
    }
}
