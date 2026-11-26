import Foundation

// 折合金额（PROTOCOL 2.2 的 `cost`、`cost_total`，3.5 的 `cca cost sync --json`）：手写数据、图怎么排（CostChart）、
// 文字（金额、token、日期、提示）、解码的宽容、看哪些文件、什么时候统计（CostPolling），和 AppModel 里的定时统计、手动刷新。

private let bobRef = AccountRef(tool: .claude, key: "工作(bob@example.com)")
private let generatedAt = 1794042000.0   // 夹具的 generated_at：2026-11-07 周六 17:00（+08）
private let me = "SHA256:" + String(repeating: "a", count: 43)
private let aliceMini = "SHA256:" + String(repeating: "b", count: 43)
private let officeImac = "SHA256:" + String(repeating: "c", count: 43)

/// 夹具是按 +08 生成的（和 WeeklyUsageTests 一样用上海时间，测试不依赖这台 Mac 的时区）
private var shanghai: Calendar {
    var c = Calendar(identifier: .gregorian)
    c.timeZone = TimeZone(identifier: "Asia/Shanghai")!
    return c
}

private func decodeCost(_ json: String) -> AccountCost? {
    try? JSONDecoder().decode(AccountCost.self, from: Data(json.utf8))
}

private func period(_ s: Double, _ e: Double, usd: Double = 0, current: Bool = false, date: String? = nil) -> CostPeriod {
    CostPeriod(startEpoch: s, endEpoch: e, date: date, current: current, usd: usd)
}

// MARK: - 手写数据

func costFixtureTests() {
    section("折合金额 · 手写数据")
    guard let s = loadFixture(StateDoc.self, "state.json") else { return }
    let claude = s.tool(.claude), codex = s.tool(.codex)
    check("Claude、Codex 都有折合金额（features.cost）", claude?.features.cost == true && codex?.features.cost == true)
    check("官方账号都能看（没登录、登错号的也能），第三方 API 账号不能、没有 cost",
          claude?.accounts.allSatisfy { $0.can.cost == !$0.isAPI } == true && claude?.accounts.filter(\.isAPI).allSatisfy { $0.cost == nil } == true)
    let bob = s.account(bobRef)?.cost
    check("bob：13 项，最后一项是本周（11 月 3 日 16:00 起）", bob?.weeks.count == 13 && bob?.currentWeek?.startEpoch == 1793692800
          && bob?.weeks.last?.current == true && bob?.weeks.filter(\.current).count == 1)
    check("bob：前面 4 项按日历切（还没有记录），之后是额度窗口", bob?.weeks.prefix(4).allSatisfy { $0.basis == .calendar } == true
          && bob?.weeks.dropFirst(4).allSatisfy { $0.basis == .quota } == true)
    check("bob：开始时还没有记录的 5 项是 partial", bob?.weeks.map(\.partial) == Array(repeating: true, count: 5) + Array(repeating: false, count: 8))
    check("bob：30 天，最后一天是今天（2026-11-07）", bob?.days.count == 30 && bob?.today?.date == "2026-11-07" && bob?.days.first?.date == "2026-10-09")
    check("bob：summary", bob?.summary == CostSummary(weeksUSD: 7957.48, currentUSD: 518.48, averageUSD: 976.02, averageWeeks: 7, daysUSD: 4284.04,
                                                     todayUSD: 44.06, averageDayUSD: 146.21, averageDays: 29, requests: 36213, unpricedRequests: 4,
                                                     weeksCacheHit: 0.9747, daysCacheHit: 0.9749),
          "\(String(describing: bob?.summary))")
    check("bob：缓存命中率（cca 按 token 算的：账号、按模型、设备的；old-mini 30 天里没有用量，没有按天的）", bob?.models?.first?.weeks.cacheHit == 0.9747
          && bob?.models?.first?.days.cacheHit == 0.9749 && bob?.models?.last?.model == nil && bob?.models?.last?.days.cacheHit == nil
          && bob?.device(aliceMini)?.models?.first?.weeks.cacheHit == 0.9747 && bob?.devices.last?.summary.weeksCacheHit == 0.9747
          && bob?.devices.last?.summary.daysCacheHit == nil, "\(String(describing: bob?.models?.map(\.weeks.cacheHit)))")
    check("bob：本周的 token", bob?.currentWeek?.tokens == CostTokens(input: 146741, output: 1128071, cacheRead: 199181482, cacheWrite: 5216937))
    check("bob：四台设备，这台在最前，撤销了的排最后", bob?.devices.map(\.name) == ["studio-mac", "alice-mini", "office-imac", "old-mini"]
          && bob?.devices.first?.isMe == true && bob?.devices.first?.id == me)
    check("bob：office-imac 很久没同步上来，old-mini 撤销了", bob?.device(officeImac)?.stale == true
          && bob?.devices.last?.revoked == true && bob?.devices.last?.stale == true)
    check("bob：每台设备的数组和 weeks、days 一样长", bob?.devices.allSatisfy { $0.weeksUSD.count == 13 && $0.daysUSD.count == 30 } == true)
    check("bob：alice-mini 本周、今天", bob?.device(aliceMini)?.weeksUSD.last == 172.76 && bob?.device(aliceMini)?.daysUSD.last == 14.36
          && bob?.device(aliceMini)?.summary.currentUSD == 172.76)
    check("bob：设备的平均（按整个账号的那几周、那几天算）", bob?.device(aliceMini)?.summary == CostSummary(
        weeksUSD: 2172.79, currentUSD: 172.76, averageUSD: 266.16, averageWeeks: 7, daysUSD: 1302.43, todayUSD: 14.36, averageDayUSD: 44.42, averageDays: 29,
        weeksCacheHit: 0.9747, daysCacheHit: 0.9749),
          "\(String(describing: bob?.device(aliceMini)?.summary))")
    check("bob：这台设备", bob?.me?.id == me && bob?.me?.name == "studio-mac" && bob?.me?.summary.averageDayUSD == 74.03)
    check("bob：设备的诊断（这台都认得出；alice-mini 命令行里还在用旧版 Claude Code，有回复认不出；旧版 cca 的设备没有）",
          bob?.me?.diag == CostDiag(cca: "2.9.0", claudeCode: "2.1.330", why: [])
          && bob?.device(aliceMini)?.diag == CostDiag(cca: "2.9.0", claudeCode: "2.1.316", claudeCodeOld: true,
                                                     why: [CostWhy(code: .oldClaudeCode, lastEpoch: 1793923200, claudeCode: "2.1.316")])
          && bob?.devices.dropFirst(2).allSatisfy { $0.diag == nil } == true, "\(String(describing: bob?.devices.map(\.diag)))")
    check("bob：没有公开价格的模型、同步过、还有一台没同步过金额", bob?.unpricedModels == ["claude-research-preview"] && bob?.syncState == .ok
          && bob?.sync?.checkedEpoch == 1794041700 && bob?.sync?.missing.map(\.name) == ["carol-air"])
    let carol = s.account(AccountRef(tool: .claude, key: "carol@example.com"))
    check("carol：没登录也有（显示上次同步来的）", carol?.login == .loggedOut && carol?.can.cost == true && carol?.cost?.syncState == .notLoggedIn
          && carol?.cost?.summary.daysUSD == 0 && carol?.cost?.summary.averageWeeks == 10)
    let dave = s.account(AccountRef(tool: .claude, key: "测试(dave@example.com)"))
    check("dave：能看，还没有记录（cost 为 null）", dave?.can.cost == true && dave?.cost == nil)
    let erin = s.account(AccountRef(tool: .claude, key: "erin@example.com"))?.cost
    check("erin：只有这台设备（local_only）", erin?.syncState == .localOnly && erin?.devices.count == 1 && erin?.weeks.count == 13)
    let grace = s.account(AccountRef(tool: .claude, key: "grace@example.com"))?.cost
    check("grace：记录都在 12 周以前（每一项都是 0），上次同步没成功", grace?.weeks.allSatisfy { $0.usd == 0 } == true
          && grace?.days.allSatisfy { $0.usd == 0 } == true && grace?.summary.averageUSD == 0 && grace?.syncState == .error
          && grace?.sync?.message == "连不上设备统计的服务器（https://192.0.2.10:18443）")
    check("grace：12 周、30 天里都没有输入，没有缓存命中率", grace?.summary.weeksCacheHit == nil && grace?.summary.daysCacheHit == nil)
    let frank = codex?.accounts.first?.cost
    check("frank（Codex）：不是哪个 Team 给的，只有这台设备（local_only），有没有公开价格的模型", frank?.syncState == .localOnly
          && frank?.sync?.hub == nil && frank?.unpricedModels == ["codex-auto-review"] && frank?.summary.currentUSD == 544.31 && frank?.weeks.count == 13)
    if let t = s.costTotal {
        check("cost_total：统计过", t.scan.state == .ready && t.scan.updatedEpoch == 1794041880)
        check("cost_total：合计", t.all.summary.currentUSD == 1324.9 && t.all.summary.weeksUSD == 18160.52 && t.all.summary.averageUSD == 2048.52
              && t.all.summary.averageDayUSD == 302.68 && t.all.summary.daysUSD == 8874.43 && t.all.summary.todayUSD == 96.83, "\(t.all.summary)")
        check("cost_total：按工具", t.all.byTool[.claude]?.currentUSD == 780.59 && t.all.byTool[.codex]?.daysUSD == 3320.53
              && t.all.byTool[.claude]?.averageDayUSD == 189.23 && t.all.byTool.count == 2)
        check("cost_total：缓存命中率（各账号的 token 加起来算的；按工具、按模型的也有）", t.all.summary.weeksCacheHit == 0.9559
              && t.all.summary.daysCacheHit == 0.9501 && t.all.byTool[.claude]?.weeksCacheHit == 0.9748 && t.all.byTool[.codex]?.daysCacheHit == 0.9107
              && t.all.models?.first?.weeks.cacheHit == 0.9748, "\(t.all.summary)")
        check("cost_total：30 天，今天", t.all.days.count == 30 && t.all.today?.usd == 96.83 && t.all.today?.date == "2026-11-07")
        check("cost_total.me：各账号里这台设备的那一份加起来", t.me?.summary == CostSummary(weeksUSD: 13830.11, currentUSD: 1100.12, averageUSD: 1516.1,
              daysUSD: 6766.87, todayUSD: 82.47, averageDayUSD: 230.5, weeksCacheHit: 0.9559, daysCacheHit: 0.9501), "\(String(describing: t.me?.summary))")
        check("cost_total.me：按工具、30 天", t.me?.byTool[.claude]?.currentUSD == 555.81 && t.me?.byTool[.codex]?.daysUSD == 3320.53
              && t.me?.days.count == 30 && t.me?.today?.usd == 82.47 && t.me?.days.first?.usd == 253.01)
        check("看哪一方：所有设备是顶层的，本机是 me", t.part(.all) == t.all && t.part(.me) == t.me)
        check("cost_total.devices：这台设备在最前（数字和 me 一样），别的按 30 天的金额从多到少，只有认不出账号的用量的 lab-studio 在最后",
              t.devices.map(\.name) == ["studio-mac", "alice-mini", "office-imac", "old-mini", "lab-studio"] && t.devices.first?.isMe == true
              && t.devices.first?.id == me && t.devices.first?.part == t.me, t.devices.map(\.name).joined(separator: ","))
        let mini = t.device(aliceMini)
        check("cost_total.devices：alice-mini 只用过 bob（数字是它在 bob 里的那一份；Codex 都是 0、没有平均、没有缓存命中率）",
              mini?.part.summary.daysUSD == 1302.43 && mini?.part.summary.averageUSD == 266.16 && mini?.part.days.last?.usd == 14.36
              && mini?.part.byTool[.claude]?.weeksUSD == 2172.79 && mini?.part.byTool[.codex]?.daysUSD == 0 && mini?.part.byTool[.codex]?.averageUSD == nil
              && mini?.part.summary.weeksCacheHit == 0.9747 && mini?.part.byTool[.codex]?.weeksCacheHit == nil,
              "\(String(describing: mini?.part.summary))")
        check("cost_total.devices：很久没同步、撤销了", t.devices.map(\.stale) == [false, false, true, true, false]
              && t.devices.map(\.revoked) == [false, false, false, true, false])
        check("看哪一方：另一台设备是 devices 里那一台的，cca 没列的设备没有", t.part(.device(aliceMini)) == mini?.part && t.part(.device("SHA256:x")) == nil)
        check("设备菜单里分隔线下面的：别的设备", t.otherDevices.map(\.name) == ["alice-mini", "office-imac", "old-mini", "lab-studio"])
        check("账号页的设备菜单：只列用过这个账号的（顺序照 cost_total）；carol 只有这台设备用过，别的设备一台都不列",
              CostChart.menuDevices(t, account: s.account(bobRef)?.cost).map(\.name) == ["alice-mini", "office-imac", "old-mini"]
              && carol?.cost != nil && CostChart.menuDevices(t, account: carol?.cost).isEmpty
              && CostChart.menuDevices(nil, account: s.account(bobRef)?.cost).isEmpty && CostChart.menuDevices(t, account: nil).isEmpty)
        if var diag = s.account(bobRef)?.cost {
            diag.devices[1].used = false
            check("账号页的设备菜单：只为诊断列出来的（used 是 false）不列", CostChart.menuDevices(t, account: diag).map(\.name) == ["office-imac", "old-mini"])
        }
        check("账号里的设备：都用过这个账号（used）", s.tools.flatMap(\.accounts).allSatisfy { $0.cost?.devices.allSatisfy(\.used) ?? true })
        check("cost_total：这台设备近 30 天认不出是哪个账号的（没有原因）", t.unattributed?.daysUSD == 12.3 && t.unattributed?.requests == 40
              && t.unattributed?.why == [] && t.unattributed?.claudeCode == "2.1.330" && t.unattributed?.claudeCodeOld == false)
        // 3.6.9 起：这台设备记不记 Claude App 里 Code 会话的账号（cca cost hook；3.7.5 起 App 不显示，接口照给）
        check("cost_total.unattributed.app_hook：开着", t.unattributed?.appHook == .on)
        // 按模型（3.6.8 起）
        let bm = bob?.models ?? []
        check("bob：按模型（列出的周期的金额从多到少，没有公开价格的只计请求数，old-mini 传来的没按模型记下，在最后）",
              bm.map(\.model) == ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-6", "claude-research-preview", nil]
              && bm.first?.weeks.usd == 4950.91 && bm.first?.weeks.requests == 22528 && bm.first?.days.usd == 2949.1
              && bm.first?.weeks.tokens == CostTokens(input: 1401104, output: 11320203, cacheRead: 2015063698, cacheWrite: 50904069)
              && bm.count == 5 && bm[3].weeks.allUnpriced && bm[3].days.requests == 1 && bm[4].weeks.usd == 772.26 && bm[4].days.isEmpty,
              bm.map(\.id).joined(separator: ","))
        check("bob：价格表里还没有的新模型（claude-haiku-5-6）带 priced_as，别的没有", bm.map(\.pricedAs) == [nil, nil, "claude-haiku-5-5", nil, nil]
              && bob?.me?.models?.first { $0.model == "claude-haiku-5-6" }?.pricedAs == "claude-haiku-5-5"
              && Texts.costEstimated(bm, granularity: .week) == "检测到新模型 claude-haiku-5-6：cca 的价格表里还没有它，按 claude-haiku-5-5 的价格估算（≈）",
              "\(bm.map(\.pricedAs))")
        check("bob：各台设备按模型的（没有 tokens；old-mini 是旧版 cca，只有没按模型记下的）",
              bob?.me?.models?.map(\.model) == ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-6", "claude-research-preview"]
              && bob?.device(aliceMini)?.models?.map(\.model) == ["claude-opus-5-5", "claude-sonnet-5-5"]
              && bob?.devices.last?.models?.map(\.model) == [nil]
              && bob?.devices.allSatisfy { $0.models?.allSatisfy { $0.weeks.tokens == nil && $0.days.tokens == nil } == true } == true)
        if let b = bob {
            check("看哪一方的按模型：所有设备是整个账号的，本机、另一台设备是那一台的，没有那台设备时是空的",
                  CostChart.models(b, scope: .all) == b.models && CostChart.models(b, scope: .me) == b.me?.models
                  && CostChart.models(b, scope: .device(aliceMini)) == b.device(aliceMini)?.models
                  && CostChart.models(b, scope: .device("SHA256:x")) == [] && CostChart.models(AccountCost(), scope: .all) == nil)
        }
        check("frank（Codex）：按模型（没有公开价格的 codex-auto-review 也列）", s.tool(.codex)?.accounts.first?.cost?.models?.map(\.model)
              == ["gpt-6.2-astra", "gpt-6.1-sol", "codex-auto-review"])
        check("cost_total：按模型（所有账号加起来，本机、各台设备的也有）", t.all.models?.map(\.model) == ["claude-opus-5-5", "gpt-6.2-astra",
              "claude-sonnet-5-5", "gpt-6.1-sol", "claude-haiku-5-6", "codex-auto-review", "claude-research-preview", nil]
              && t.all.models?.first?.weeks.usd == 8073.69 && t.me?.models?.count == 7 && t.devices.first?.part.models == t.me?.models
              && t.device(aliceMini)?.part.models?.map(\.model) == ["claude-opus-5-5", "claude-sonnet-5-5"])
        // 认不出账号（3.6.8 起，所有设备的）
        let oc = t.unattributed?.cost, lab = "f0f0f0f0f0f0f0f0"
        check("认不出账号的：所有设备的（按日历周，三台设备，这台在最前），按模型，还有两台没传",
              oc?.summary.weeksUSD == 484.94 && oc?.summary.daysUSD == 175.1 && oc?.summary.requests == 3537
              && oc?.weeks.allSatisfy { $0.basis == .calendar } == true && oc?.weeks.count == 13
              && oc?.devices.map(\.name) == ["studio-mac", "lab-studio", "alice-mini"] && oc?.me?.id == me
              && oc?.models?.map(\.model) == ["claude-opus-5-5", "claude-sonnet-5-5", nil]
              && t.unattributed?.unsynced.map(\.name) == ["office-imac", "old-mini"], "\(String(describing: oc?.summary))")
        check("只有认不出账号的用量的 lab-studio：设备菜单里在最后，账号的几项都是 0、按模型是空的，账号页的菜单里没有",
              t.devices.last?.id == lab && t.device(lab)?.part.summary.weeksUSD == 0 && t.device(lab)?.part.models == []
              && !CostChart.menuDevices(t, account: bob).contains { $0.id == lab })
        check("「认不出账号」那一组看谁的：所有设备、本机、有这种用量的另一台设备；没有这种用量的设备、旧版 cca 不列",
              CostChart.orphan(t, scope: .all) == oc && CostChart.orphan(t, scope: .me) == oc && CostChart.orphan(t, scope: .device(lab)) == oc
              && CostChart.orphan(t, scope: .device(officeImac)) == nil && CostChart.orphan(nil, scope: .all) == nil
              && CostChart.listsOrphanDevices(oc, scope: .all) && !CostChart.listsOrphanDevices(oc, scope: .me)
              && !CostChart.listsOrphanDevices(oc, scope: .device(lab)))
        check("cost_total：价格表的日期", t.pricesDate == "2026-11-01" && t.currency == "USD")
    } else {
        check("state.json 有 cost_total", false)
    }
    if let n = loadFixture(StateDoc.self, "state_nocodex.json") {
        check("旧版 cca（没有 cost_total、features.cost、can.cost、cost）：都当作没有", n.costTotal == nil
              && n.tools.allSatisfy { !$0.features.cost && $0.accounts.allSatisfy { !$0.can.cost && $0.cost == nil } })
    }
    if let a = loadFixture(StateDoc.self, "state_agy.json") {
        // 手写的这一份是 3.7.2 以前的样子（没有 features.cost）；回归里 cca 的真实输出（3.7.2 起）有，登录着的账号 can.cost
        check("Antigravity 的折合金额：can.cost 只在 features.cost 为真时才有", a.tool(.agy).map { t in t.accounts.allSatisfy { !$0.can.cost || t.features.cost } } == true)
    }
    if let x = loadFixture(StateDoc.self, "state_cost.json") {
        let alice = x.account(AccountRef(tool: .claude, key: "alice@example.com"))?.cost
        check("state_cost：只有这台设备，没开设备统计时 id 是 local、名字是主机名", alice?.devices.count == 1 && alice?.devices.first?.id == "local"
              && alice?.devices.first?.name == "test-mac" && alice?.syncState == .localOnly)
        check("state_cost：设备菜单只有这台设备（id 是 local）", x.costTotal?.devices.map(\.id) == ["local"]
              && x.costTotal?.otherDevices.isEmpty == true && x.costTotal?.devices.first?.part == x.costTotal?.me)
        check("state_cost（3.6.7 的样子）：没有按模型的、所有设备的认不出账号", alice?.models == nil && x.costTotal?.all.models == nil
              && x.costTotal?.unattributed?.cost == nil && x.costTotal?.unattributed?.unsynced.isEmpty == true)
        check("state_cost：开始时还没有记录的两项是 partial、第一项是 0", alice?.weeks.prefix(2).allSatisfy(\.partial) == true
              && alice?.weeks.first?.usd == 0 && alice?.weeks.count == 13)
        check("state_cost：Codex 账号还没有记录，Codex 的平均是 null", x.account(AccountRef(tool: .codex, key: "dave@example.com"))?.cost == nil
              && x.costTotal?.all.byTool[.codex]?.averageUSD == nil && x.costTotal?.all.byTool[.codex]?.currentUSD == 0)
        let u = x.costTotal?.unattributed
        check("state_cost（3.6.7 的样子）：没有 app_hook", u?.appHook == nil && u != nil)
        check("app_hook 不认识的值：不当作认识的", CostAppHook(rawValue: "later").isKnown == false)
        check("state_cost：cca 给的认不出的原因、命令行里的 Claude Code 版本照样解码（App 不显示）；这台设备的诊断是同一份",
              u?.why.map(\.code) == [.claudeApp, .oldClaudeCode] && u?.why.first?.claudeCode == nil && u?.claudeCode == "2.1.316" && u?.claudeCodeOld == true
              && alice?.me?.diag?.why == u?.why && alice?.me?.diag?.claudeCodeOld == true)
        check("state_cost：合计下面只说有用量认不出账号、没算进来，不说原因、不提醒更新；只看本机时什么都不说",
              Texts.costUnattributed(u, currency: "USD") == "近 30 天另有 $1.25 认不出是哪个账号的，没算进来。"
              && alice.map { Texts.costNotes($0, scope: .me, now: 1794042000) } == [],
              "\(String(describing: alice.map { Texts.costNotes($0, scope: .me, now: 1794042000) }))")
    }
    if let x = loadFixture(StateDoc.self, "state_cost_sync.json") {
        let alice = x.account(AccountRef(tool: .claude, key: "alice@example.com"))?.cost
        check("state_cost_sync：三台设备（这台、dan-air、撤销了的 old-air），还有一台没同步过金额",
              alice?.devices.map(\.name) == ["test-mac", "dan-air", "old-air"] && alice?.devices.last?.revoked == true
              && alice?.sync?.missing.map(\.name) == ["eve-mini"] && alice?.syncState == .ok)
        check("state_cost_sync：设备菜单：这台、dan-air、撤销了的 old-air", x.costTotal?.devices.map(\.name) == ["test-mac", "dan-air", "old-air"]
              && x.costTotal?.devices.map(\.revoked) == [false, false, true]
              && x.costTotal?.otherDevices.map(Texts.costMenuName) == ["dan-air", "old-air（已撤销）"])
        check("state_cost_sync：dan-air 带着诊断（cca 照给），不说成「有用量没算进来」；没同步过金额的设备照旧说",
              alice?.devices.map { $0.diag?.why.isEmpty == false } == [false, true, false]
              && alice.map { Texts.costNotes($0, now: 1794042000).filter { $0.text.contains("算进来") } } == [
                CostNote(text: "还有 1 台设备没有算进来（eve-mini，可能 cca 太旧）", warning: true)],
              "\(String(describing: alice.map { Texts.costNotes($0, now: 1794042000) }))")
    }
    if let d = fixture("cost_sync.json"), let doc = try? DocumentDecoder.decode(CostSyncDoc.self, from: d) {
        check("cost_sync.json", doc.kind == "cost_sync" && doc.scan?.state == .ready && doc.scan?.files == 190 && doc.scan?.records == 120
              && doc.scan?.first == false && doc.sync?.state == .ok && doc.sync?.checkedEpoch == 1794042000)
    } else {
        check("cost_sync.json 解码", false)
    }
}

// MARK: - 纯函数

func costUnitTests() {
    section("折合金额 · 图")
    check("纵轴：最高的那根到九成以内，取整齐的数", CostChart.niceTop(518.48) == 600 && CostChart.niceTop(1190.81) == 1500
          && CostChart.niceTop(91) == 150 && CostChart.niceTop(9) == 10 && CostChart.niceTop(90) == 100 && CostChart.niceTop(2.2) == 2.5,
          "\(CostChart.niceTop(518.48)) \(CostChart.niceTop(1190.81)) \(CostChart.niceTop(91)) \(CostChart.niceTop(9)) \(CostChart.niceTop(90)) \(CostChart.niceTop(2.2))")
    check("纵轴：最少到 $1，都是 0 也是 $1", CostChart.niceTop(0) == 1 && CostChart.niceTop(0.004) == 1 && CostChart.niceTop(0.5) == 1
          && CostChart.niceTop(.nan) == 1 && CostChart.niceTop(-3) == 1)
    check("纵轴：最高的那根不超过九成", [0.9, 1.7, 13, 47.5, 99, 101, 333, 4321, 99_999].allSatisfy { v in
        let top = CostChart.niceTop(v)
        return v <= top * CostChart.headroom + 1e-9 && top < v / CostChart.headroom * 2.01 + 1
    })
    check("刻度：0、一半、最高", CostChart.ticks(top: 600) == [0, 300, 600] && CostChart.ticks(top: 2.5) == [0, 1.25, 2.5])
    check("几行共用一个纵轴：取所有行里最高的", CostChart.maxUSD([[CostBar(period: period(0, 1), usd: 3)], [CostBar(period: period(1, 2), usd: 7)], []]) == 7
          && CostChart.maxUSD([]) == 0)
    let now = generatedAt
    let weeks = [period(now - 20 * 86400, now - 13 * 86400, usd: 5), period(now - 13 * 86400, now - 6 * 86400, usd: 6),
                 period(now - 6 * 86400, now + 86400, usd: 2, current: true)]
    let w = CostChart.domain([weeks], granularity: .week, now: now)
    check("横轴（按周）：最右边是这一周结束的时间，往左 13 周", w.upperBound == now + 86400 && w.upperBound - w.lowerBound == 13 * 7 * 86400)
    let d = CostChart.domain([[period(now - 3600, now + 1800, current: true)]], granularity: .day, now: now)
    check("横轴（按天）：最右边是今天结束，往左 30 天", d.upperBound == now + 1800 && d.upperBound - d.lowerBound == 30 * 86400)
    check("横轴：都结束了时最右边是现在；几行取最晚的", CostChart.domain([[period(0, 10)]], granularity: .week, now: 100).upperBound == 100
          && CostChart.domain([[period(0, 150)], [period(0, 180)]], granularity: .week, now: 100).upperBound == 180)
    let dom = 0.0...100.0
    check("一项在横轴上的位置：按时间，超出的截掉", CostChart.slot(start: 10, end: 20, domain: dom, width: 200).map { [$0.x0, $0.x1] } == [20, 40]
          && CostChart.slot(start: -50, end: 5, domain: dom, width: 200).map { [$0.x0, $0.x1] } == [0, 10])
    check("看不见、不到 1 点宽的不画", CostChart.slot(start: 120, end: 130, domain: dom, width: 200) == nil
          && CostChart.slot(start: 10, end: 10.2, domain: dom, width: 200) == nil && CostChart.slot(start: 1, end: 2, domain: 5...5, width: 200) == nil)
    check("按天：日期之间放得下「11/30」（1、2、7、14 天里最小的）", CostChart.dayLabelStride(slotWidth: 40) == 1 && CostChart.dayLabelStride(slotWidth: 20) == 2
          && CostChart.dayLabelStride(slotWidth: 10) == 7 && CostChart.dayLabelStride(slotWidth: 3) == 14 && CostChart.dayLabelStride(slotWidth: 1) == 14)
    let shown = (0..<30).filter { CostChart.showsDayLabel(index: $0, count: 30, stride: 7) }
    check("按天：从今天往前数，每隔几天写一个", shown == [1, 8, 15, 22, 29], "\(shown)")
    check("下标不对的不写", !CostChart.showsDayLabel(index: 30, count: 30, stride: 1) && !CostChart.showsDayLabel(index: -1, count: 30, stride: 1)
          && !CostChart.showsDayLabel(index: 0, count: 30, stride: 0))

    // 用量统计页表头的日期（以前不管多宽都每 2 周一个：1080 宽的窗口里挤成「10/30今天」）
    let wk = 7 * 86400.0
    let dom13 = (now - 13 * wk + 2 * 86400)...(now + 2 * 86400)   // 这一周还有两天结束
    func weeksApart(_ t: [StatsTick]) -> [Double] { zip(t, t.dropFirst()).map { ($0.epoch - $1.epoch) / wk } }
    func roomy(_ t: [StatsTick]) -> Bool { zip(t, t.dropFirst()).allSatisfy { $0.x - $1.x >= CostChart.dayLabelMinWidth } }
    let wide = CostChart.statsTicks(domain: dom13, now: now, width: 600)
    check("表头（宽）：从今天往前每 2 周一个日期", wide.first?.epoch == now && weeksApart(wide) == Array(repeating: 2, count: 6) && roomy(wide),
          "\(wide)")
    let narrow = CostChart.statsTicks(domain: dom13, now: now, width: 185)
    check("表头（窄：小图那一栏 185 点，和 1080 宽的窗口差不多）：今天挨着右边、往里挪了，这时隔 2 周、3 周都得跳过一个，所以隔 4 周，日期之间都放得下",
          narrow.first?.epoch == now && weeksApart(narrow) == [4, 4, 4] && roomy(narrow), "\(narrow)")
    check("表头：字不出这一栏（挨着两头的往里挪到 18 点）", (wide + narrow).allSatisfy { $0.x >= 18 }
          && wide.allSatisfy { $0.x <= 582 } && narrow.allSatisfy { $0.x <= 167 })
    check("表头（很窄）：放不下第二个时只写今天；一个都放不下、没有时间范围时不写",
          CostChart.statsTicks(domain: dom13, now: now, width: 60).map(\.epoch) == [now]
          && CostChart.statsTicks(domain: dom13, now: now, width: 30).isEmpty && CostChart.statsTicks(domain: 5...5, now: 5, width: 600).isEmpty)

    // 看谁的：所有设备加起来（整个账号）、只看这台设备（本机），或者另一台设备；折合金额页各台设备那一行看那一台
    check("看谁的记在设置里：all、me、d:设备 id；认不出的不认", [CostScope.all, .me, .device(aliceMini)].map(\.rawValue) == ["all", "me", "d:" + aliceMini]
          && ["all", "me", "d:" + aliceMini, "d:", "", "device", "ALL"].map(CostScope.init(rawValue:)) == [.all, .me, .device(aliceMini), nil, nil, nil, nil]
          && CostScope.device("x").isOtherDevice && !CostScope.me.isOtherDevice && !CostScope.all.isOtherDevice)
    if let bob = loadFixture(StateDoc.self, "state.json")?.account(bobRef)?.cost {
        check("所有设备：柱子是 weeks、days 的 usd", CostChart.bars(bob, granularity: .week, scope: .all).map(\.usd) == bob.weeks.map(\.usd)
              && CostChart.bars(bob, granularity: .day, scope: .all).map(\.usd) == bob.days.map(\.usd))
        check("本机：柱子是这台设备的 weeks_usd、days_usd", CostChart.bars(bob, granularity: .week, scope: .me).map(\.usd) == bob.me?.weeksUSD
              && CostChart.bars(bob, granularity: .day, scope: .me).map(\.usd) == bob.me?.daysUSD)
        check("一台设备（折合金额页那一行）：它的", CostChart.bars(bob, granularity: .week, device: bob.device(aliceMini)).map(\.usd) == bob.device(aliceMini)?.weeksUSD
              && CostChart.bars(bob, granularity: .day, device: bob.device(aliceMini)).map(\.usd) == bob.device(aliceMini)?.daysUSD)
        check("柱子跟着周期（日期、partial 是账号的）", CostChart.bars(bob, granularity: .day, scope: .me).map(\.period) == bob.days
              && CostChart.bars(bob, granularity: .week, device: bob.device(aliceMini)).map(\.period) == bob.weeks)
        check("统计：所有设备的、本机的", CostChart.summary(bob, scope: .all) == bob.summary && CostChart.summary(bob, scope: .me) == bob.me?.summary)
        let others = AccountCost(weeks: bob.weeks, days: bob.days, summary: bob.summary, devices: bob.devices.filter { !$0.isMe })
        check("这台设备没有这个账号的记录：本机的柱子都是 0、统计都是 0、没有平均", others.me == nil
              && CostChart.bars(others, granularity: .week, scope: .me).allSatisfy { $0.usd == 0 }
              && CostChart.bars(others, granularity: .week, scope: .me).count == bob.weeks.count
              && CostChart.summary(others, scope: .me) == CostSummary() && CostChart.bars(others, granularity: .day, device: nil).allSatisfy { $0.usd == 0 })
        check("一台以上有记录的设备才列出来", CostChart.listsDevices(bob) && !CostChart.listsDevices(nil)
              && !CostChart.listsDevices(AccountCost(devices: Array(bob.devices.prefix(1)))))
        check("另一台设备：柱子、统计是那一台的（和折合金额页那一行一样）", CostChart.bars(bob, granularity: .week, scope: .device(aliceMini)).map(\.usd)
              == bob.device(aliceMini)?.weeksUSD && CostChart.summary(bob, scope: .device(aliceMini)) == bob.device(aliceMini)?.summary
              && CostChart.device(bob, scope: .device(aliceMini))?.name == "alice-mini" && CostChart.device(bob, scope: .all) == nil
              && CostChart.device(bob, scope: .me)?.isMe == true)
        check("另一台设备没用过这个账号：柱子都是 0、统计都是 0", CostChart.bars(bob, granularity: .day, scope: .device("SHA256:x")).allSatisfy { $0.usd == 0 }
              && CostChart.bars(bob, granularity: .day, scope: .device("SHA256:x")).count == 30 && CostChart.summary(bob, scope: .device("SHA256:x")) == CostSummary())
        var diagOnly = bob
        diagOnly.devices[1].used = false
        check("折合金额页看另一台设备时列不列这个账号：那台设备用过它（只为诊断列出来的不算）；所有设备、本机都列",
              CostChart.listed(bob, scope: .device(aliceMini)) && !CostChart.listed(diagOnly, scope: .device(aliceMini))
              && !CostChart.listed(bob, scope: .device("SHA256:x")) && !CostChart.listed(nil, scope: .device(aliceMini))
              && CostChart.listed(nil, scope: .all) && CostChart.listed(nil, scope: .me) && CostChart.listed(diagOnly, scope: .me))

        // 账号页「每周用量」的图右边留出和「折合金额」的大图的纵轴一样宽的一栏（同样的刻度撑开）：两张图的绘图区一样宽
        let gutter = CostChart.gutterLabels(bob, granularity: .week, scope: .all)
        check("每周用量的图右边那一栏：和折合金额的大图右边的刻度一样", gutter == ["$0", "$750", "$1,500"]
              && gutter == CostChart.axisLabels(top: CostChart.niceTop(CostChart.maxUSD([CostChart.bars(bob, granularity: .week, scope: .all)])),
                                                currency: bob.currency), "\(gutter)")
        let mine = bob.me?.weeksUSD.max() ?? 0
        check("跟着折合金额看的那一方（本机有它自己的纵轴）、按日还是按周",
              CostChart.gutterLabels(bob, granularity: .week, scope: .me) == CostChart.axisLabels(top: CostChart.niceTop(mine), currency: "USD")
              && CostChart.gutterLabels(bob, granularity: .day, scope: .all)
                == CostChart.axisLabels(top: CostChart.niceTop(bob.days.map(\.usd).max() ?? 0), currency: "USD"))
        let zero = AccountCost(weeks: bob.weeks.map { period($0.startEpoch, $0.endEpoch) })
        check("折合金额没画大图（还没有记录、都是 0、看本机而这台设备没有记录）时不留", CostChart.gutterLabels(nil, granularity: .week, scope: .all).isEmpty
              && !CostChart.charted(CostChart.bars(zero, granularity: .week, scope: .all))
              && CostChart.gutterLabels(zero, granularity: .week, scope: .all).isEmpty
              && CostChart.gutterLabels(others, granularity: .week, scope: .me).isEmpty)
    }

    section("折合金额 · 文字")
    check("金额：0、不到一分", Texts.money(0) == "$0" && Texts.money(0.004) == "<$0.01" && Texts.money(0.01) == "$0.01"
          && Texts.money(-3) == "$0" && Texts.money(.nan) == "$0" && Texts.money(.infinity) == "$0")
    check("金额：不到 100 带两位小数", Texts.money(12.346) == "$12.35" && Texts.money(5) == "$5.00" && Texts.money(99.994) == "$99.99",
          [Texts.money(12.346), Texts.money(5), Texts.money(99.994)].joined(separator: " "))
    check("金额：100 起取整、加千分位", Texts.money(99.996) == "$100" && Texts.money(123.4) == "$123" && Texts.money(1234.5) == "$1,235"
          && Texts.money(7957.48) == "$7,957" && Texts.money(1_234_567.8) == "$1,234,568")
    check("金额：别的货币写代码", Texts.money(12.346, currency: "EUR") == "EUR 12.35" && Texts.money(1, currency: "usd") == "$1.00")
    check("纵轴的刻度：去掉多余的 0", Texts.moneyAxis(0) == "$0" && Texts.moneyAxis(2.5) == "$2.5" && Texts.moneyAxis(0.25) == "$0.25"
          && Texts.moneyAxis(300) == "$300" && Texts.moneyAxis(1500) == "$1,500" && Texts.moneyAxis(1.5) == "$1.5" && Texts.moneyAxis(1250) == "$1,250",
          [Texts.moneyAxis(2.5), Texts.moneyAxis(0.25), Texts.moneyAxis(300), Texts.moneyAxis(1500)].joined(separator: " "))
    check("千分位", Texts.grouped(0) == "0" && Texts.grouped(999) == "999" && Texts.grouped(1000) == "1,000"
          && Texts.grouped(1_234_567) == "1,234,567" && Texts.grouped(-1234) == "-1,234")
    let toks = [999, 1000, 1234, 99_949, 99_950, 999_499, 999_500, 210_334_112, 4_011_223, 1_200_000_000].map(Texts.tokens)
    check("token 数：K、M、B，四舍五入到一千的进一级", toks == ["999", "1K", "1.2K", "99.9K", "100K", "999K", "1M", "210M", "4M", "1.2B"],
          toks.joined(separator: " "))
    check("一项的 token", Texts.costTokens(CostTokens(input: 120334, output: 802112, cacheRead: 210334112, cacheWrite: 4011223))
          == "输入 120K · 输出 802K · 缓存读 210M · 缓存写 4M")
    check("cca 给的日期", Texts.costDateParts("2026-11-07").map { [$0.year, $0.month, $0.day] } == [2026, 11, 7]
          && Texts.costDateParts("2026-13-01") == nil && Texts.costDateParts("11/07") == nil && Texts.costDateParts("") == nil
          && Texts.costPricesDate("2026-11-01") == "2026 年 11 月 1 日" && Texts.costPricesDate("最近") == "最近")

    let sh = shanghai
    guard let s = loadFixture(StateDoc.self, "state.json"), let bob = s.account(bobRef)?.cost,
          let carol = s.account(AccountRef(tool: .claude, key: "carol@example.com"))?.cost,
          let grace = s.account(AccountRef(tool: .claude, key: "grace@example.com"))?.cost,
          let erin = s.account(AccountRef(tool: .claude, key: "erin@example.com"))?.cost,
          let frank = s.tool(.codex)?.accounts.first?.cost else {
        check("夹具里有 bob、carol、grace、erin、frank 的折合金额", false)
        return
    }
    let range = { (p: CostPeriod, g: CostGranularity) in Texts.costRange(p, granularity: g, now: now, calendar: sh) }
    check("起止：额度窗口写到结束那天", range(bob.weeks[12], .week) == "11 月 3 日–10 日", range(bob.weeks[12], .week))
    check("起止：在零点结束的（按日历切的周）写到前一天", range(bob.weeks[0], .week) == "8 月 17 日–23 日", range(bob.weeks[0], .week))
    check("起止：跨月", range(carol.weeks[2], .week) == "8 月 31 日–9 月 6 日" && range(bob.weeks[7], .week) == "9 月 29 日–10 月 6 日",
          range(carol.weeks[2], .week) + " " + range(bob.weeks[7], .week))
    check("一天：日期和星期几", range(bob.days[29], .day) == "11 月 7 日 周六" && range(bob.days[0], .day) == "10 月 9 日 周五",
          range(bob.days[29], .day) + " " + range(bob.days[0], .day))
    var buddhist = Calendar(identifier: .buddhist)
    buddhist.timeZone = sh.timeZone
    let buddhistDay = Texts.costRange(bob.days[29], granularity: .day, now: now, calendar: buddhist)
    check("一天：系统日历是佛历时，cca 给的公历日期也认得对（不当成佛历的 2026 年）", buddhistDay == "11 月 7 日 周六", buddhistDay)
    check("不是今年的带年份", Texts.costRange(bob.weeks[12], granularity: .week, now: now + 400 * 86400, calendar: sh) == "2026 年 11 月 3 日–10 日")
    check("不到一天的一项（按日历切的零头）", Texts.costRange(period(1788710400, 1788854400), granularity: .week, now: now, calendar: sh) == "9 月 7 日–8 日")
    check("柱子下面的日期：开始那天，这一周写本周、今天写今天",
          Texts.costLabel(bob.weeks[11], granularity: .week, calendar: sh) == "10/27" && Texts.costLabel(bob.weeks[12], granularity: .week, calendar: sh) == "本周"
          && Texts.costLabel(bob.days[0], granularity: .day, calendar: sh) == "10/9" && Texts.costLabel(bob.days[29], granularity: .day, calendar: sh) == "今天")
    check("没有 date 的一天按时间算", Texts.costLabel(period(1793980800, 1794067200 - 86400 + 86400), granularity: .day, calendar: sh) == "11/7")
    let bars = CostChart.bars(bob, granularity: .week, scope: .all)
    let detail = { (i: Int, scope: CostScope, g: CostGranularity) -> String in
        let list = CostChart.bars(bob, granularity: g, scope: scope)
        return Texts.costBarDetail(list[i], granularity: g, scope: scope, currency: "USD", now: now, calendar: sh)
    }
    check("指着本周", detail(12, .all, .week) == "本周到现在（11 月 3 日–10 日）：$518 · 2,327 次请求", detail(12, .all, .week))
    check("指着开始时还没有记录的一周：补一句", detail(4, .all, .week) == "9 月 8 日–15 日：$607 · 2,768 次请求；这一项开始时还没有记录，可能偏小",
          detail(4, .all, .week))
    check("有没有公开价格的模型的一周：补一句", detail(7, .all, .week) == "9 月 29 日–10 月 6 日：$785 · 3,609 次请求；其中 3 次请求的模型没有公开价格，没算钱",
          detail(7, .all, .week))
    check("还没有记录的一周：$0，不写请求数", detail(0, .all, .week) == "8 月 17 日–23 日：$0；这一项开始时还没有记录，可能偏小", detail(0, .all, .week))
    check("本机：这台设备的金额，没有请求数", detail(12, .me, .week) == "本周到现在（11 月 3 日–10 日）：$294" && detail(29, .me, .day) == "今天到现在：$29.70",
          detail(12, .me, .week) + " | " + detail(29, .me, .day))
    check("指着今天、别的一天", detail(29, .all, .day) == "今天到现在：$44.06 · 191 次请求"
          && detail(0, .all, .day) == "10 月 9 日 周五：$185 · 864 次请求", detail(29, .all, .day) + " | " + detail(0, .all, .day))
    check("柱子的金额是 cca 给的", bars.map(\.usd) == bob.weeks.map(\.usd))
    check("图下面那一行（按周）", Texts.costCaption(bob.summary, granularity: .week, currency: "USD") == "本周 $518 · 平均每周 $976（7 周） · 近 12 周合计 $7,957 · 缓存命中 97.4%",
          Texts.costCaption(bob.summary, granularity: .week, currency: "USD"))
    check("图下面那一行（按天）", Texts.costCaption(bob.summary, granularity: .day, currency: "USD") == "今天 $44.06 · 平均每天 $146（29 天） · 近 30 天合计 $4,284 · 缓存命中 97.4%",
          Texts.costCaption(bob.summary, granularity: .day, currency: "USD"))
    check("没有平均（一周、一天都没过完）时不写平均", Texts.costCaption(CostSummary(weeksUSD: 3, currentUSD: 3), granularity: .week, currency: "USD")
          == "本周 $3.00 · 近 12 周合计 $3.00"
          && Texts.costCaption(CostSummary(daysUSD: 3, todayUSD: 3), granularity: .day, currency: "USD") == "今天 $3.00 · 近 30 天合计 $3.00")
    check("一台设备的平均也写几周、几天（按整个账号的那几周、那几天算）", Texts.costCaption(bob.device(aliceMini)!.summary, granularity: .week, currency: "USD")
          == "本周 $173 · 平均每周 $266（7 周） · 近 12 周合计 $2,173 · 缓存命中 97.4%"
          && Texts.costCaption(bob.device(aliceMini)!.summary, granularity: .day, currency: "USD") == "今天 $14.36 · 平均每天 $44.42（29 天） · 近 30 天合计 $1,302 · 缓存命中 97.4%",
          Texts.costCaption(bob.device(aliceMini)!.summary, granularity: .day, currency: "USD"))
    check("旧版 cca 没给几周、平均每天：平均不写几周，按天不写平均", Texts.costCaption(CostSummary(weeksUSD: 10, currentUSD: 1, averageUSD: 3), granularity: .week,
                                                                    currency: "USD") == "本周 $1.00 · 平均每周 $3.00 · 近 12 周合计 $10.00"
          && Texts.costCaption(CostSummary(weeksUSD: 10, currentUSD: 1, averageUSD: 3, daysUSD: 5, todayUSD: 1), granularity: .day, currency: "USD")
            == "今天 $1.00 · 近 30 天合计 $5.00")
    check("缓存命中率：千分之一往下取（没到 100% 的不写成 100%），整的不写小数；cca 没给时没有", Texts.costCacheHit(0.9747) == "97.4%"
          && Texts.costCacheHit(0.973) == "97.3%" && Texts.costCacheHit(0.97) == "97%" && Texts.costCacheHit(0.29) == "29%"
          && Texts.costCacheHit(0.9999) == "99.9%" && Texts.costCacheHit(1) == "100%" && Texts.costCacheHit(0.0004) == "0%"
          && Texts.costCacheHit(nil) == nil && Texts.costCacheHit(.nan) == nil, String(describing: Texts.costCacheHit(0.973)))
    check("图下面那一行：缓存命中率写在最后，这一段没有（cca 没给、没有输入）时不写", Texts.costCaption(CostSummary(weeksUSD: 3, currentUSD: 3,
          weeksCacheHit: 0.5), granularity: .week, currency: "USD") == "本周 $3.00 · 近 12 周合计 $3.00 · 缓存命中 50%"
          && Texts.costCaption(CostSummary(daysUSD: 3, todayUSD: 3, weeksCacheHit: 0.5), granularity: .day, currency: "USD") == "今天 $3.00 · 近 30 天合计 $3.00")
    check("价格的小字：按工具说是谁的价格", Texts.costFootnote([.claude], pricesDate: "2026-11-01")
          == "按 Anthropic 官方 API 的价格折算（价格表 2026 年 11 月 1 日），不是实际花费：订阅不按这个收费。"
          && Texts.costFootnote([.codex], pricesDate: nil) == "按 OpenAI 官方 API 的价格折算，不是实际花费：订阅不按这个收费。"
          && Texts.costFootnote([.claude, .codex, .claude], pricesDate: "2026-11-01").hasPrefix("按 Anthropic、OpenAI 官方 API 的价格折算（价格表 2026 年 11 月 1 日）")
          && Texts.costFootnote([], pricesDate: nil) == "按官方 API 的价格折算，不是实际花费：订阅不按这个收费。",
          Texts.costFootnote([], pricesDate: nil))

    let notes = { (c: AccountCost) in Texts.costNotes(c, now: now, calendar: sh) }
    check("同步过：还有没同步过金额的设备、很久没同步上来的设备（撤销了的不说）；有回复认不出账号的设备不说", notes(bob) == [
        CostNote(text: "还有 1 台设备没有算进来（carol-air，可能 cca 太旧）", warning: true),
        CostNote(text: "office-imac 周四 15:00 以后的还没同步上来", warning: true),
    ], "\(notes(bob))")
    let meNotes = { (c: AccountCost) in Texts.costNotes(c, scope: .me, now: now, calendar: sh) }
    check("只看本机：什么都不说（这台设备自己的数字不经同步，别的设备没算进来、没同步上来不影响它）",
          meNotes(bob).isEmpty && meNotes(grace).isEmpty && meNotes(frank).isEmpty && meNotes(carol).isEmpty && meNotes(erin).isEmpty)
    let deviceNotes = { (c: AccountCost, id: String) in Texts.costNotes(c, scope: .device(id), now: now, calendar: sh) }
    check("只看另一台设备：只说那一台很久没同步上来（同步的状态、别的设备、诊断不说）",
          deviceNotes(bob, officeImac) == [CostNote(text: "office-imac 周四 15:00 以后的还没同步上来", warning: true)]
          && deviceNotes(bob, aliceMini).isEmpty
          && deviceNotes(bob, "SHA256:" + String(repeating: "e", count: 43)).isEmpty && deviceNotes(bob, "SHA256:x").isEmpty && deviceNotes(grace, aliceMini).isEmpty,
          "\(deviceNotes(bob, aliceMini))")
    if let t = s.costTotal {
        check("一根柱子也没有时说的：所有设备不说是谁，本机「这台设备」，另一台设备是它的名字（cca 没列的不说）",
              Texts.costNoUsage(.all, total: t, granularity: .week) == "最近 12 周没有用量。"
              && Texts.costNoUsage(.me, total: t, granularity: .day) == "这台设备最近 30 天没有用量。"
              && Texts.costNoUsage(.device(aliceMini), total: t, granularity: .week) == "alice-mini 最近 12 周没有用量。"
              && Texts.costNoUsage(.device("SHA256:x"), total: t, granularity: .day) == "最近 30 天没有用量。")
        check("折合金额页一个账号都不列时", Texts.costNoAccounts(.device(aliceMini), total: t) == "alice-mini 最近没有用过这里的账号。"
              && Texts.costNoAccounts(.device("SHA256:x"), total: t) == "那台设备最近没有用过这里的账号。"
              && Texts.costNoAccounts(.all, total: t).hasPrefix("还没有 Claude、ChatGPT 账号。") && Texts.costNoAccounts(.me, total: nil) == Texts.costNoAccounts(.all, total: t))
        check("合计那一块的读屏标签", Texts.costTotalLabel(.all, total: t) == "所有账号的折合金额"
              && Texts.costTotalLabel(.me, total: t) == "所有账号在这台设备上的折合金额"
              && Texts.costTotalLabel(.device(aliceMini), total: t) == "所有账号在 alice-mini 上的折合金额")
        check("设备菜单里的叫法：撤销了的、很久没同步上来的标出来", t.otherDevices.map(Texts.costMenuName)
              == ["alice-mini", "office-imac（很久没同步）", "old-mini（已撤销）", "lab-studio"])
        check("只有认不出账号的用量的设备：折合金额页一个账号都不列", Texts.costNoAccounts(.device("f0f0f0f0f0f0f0f0"), total: t) == "lab-studio 最近没有用过这里的账号。")
    }
    let syncFailed = decodeCost(#"{"weeks":[],"days":[],"devices":[{"id":"m","name":"m","me":true},{"id":"o","name":"o"}],"#
                                + #""sync":{"state":"error","message":"x","missing":[{"name":"z"}]}}"#)
    check("同步没成功、还有设备没算进来：看所有设备时说，只看本机时不说",
          syncFailed.map { notes($0) } == [CostNote(text: "上次同步没成功：x", warning: true),
                                          CostNote(text: "还有 1 台设备没有算进来（z，可能 cca 太旧）", warning: true)]
          && syncFailed.map { meNotes($0) }?.isEmpty == true, "\(String(describing: syncFailed.map { notes($0) }))")
    check("没登录这个账号", notes(carol) == [CostNote(text: "这台设备没登录这个账号，显示的是上次同步来的")], "\(notes(carol))")
    check("上次同步没成功（橙色，带原因）", notes(grace) == [CostNote(text: "上次同步没成功：连不上设备统计的服务器（https://192.0.2.10:18443）", warning: true)])
    let localOnly = [CostNote(text: "只算了这台设备：这个账号不是哪个 Team 给的，或者这台设备用不了那个 Team 的服务器")]
    check("只有这台设备（灰字：所有设备和本机一样）", notes(erin) == localOnly && notes(frank) == localOnly, "\(notes(erin))")
    let oldServer = decodeCost(#"{"weeks":[],"days":[],"sync":{"state":"old_server","message":"维护者的设备统计服务器还不支持同步折合金额","hub":"maintainer","missing":[]}}"#)
    check("3.6.0 以前的 cca 给的 old_server（维护者的服务器还不支持）：当没同步成，照 cca 给的原因说；维护者的服务器不认识",
          oldServer?.syncState == .error && oldServer?.sync?.hub == nil
          && oldServer.map({ notes($0) }) == [CostNote(text: "上次同步没成功：维护者的设备统计服务器还不支持同步折合金额", warning: true)],
          "\(String(describing: oldServer.map { notes($0) }))")
    let pending = decodeCost(#"{"weeks":[],"days":[],"sync":{"state":"pending","missing":[]}}"#)
    let weird = decodeCost(#"{"weeks":[],"days":[],"sync":{"state":"later","missing":[{"name":"a"},{"name":"b"},{"name":"c"},{"name":"d"},{"name":""}]}}"#)
    let off = decodeCost(#"{"weeks":[],"days":[],"sync":{"state":"off","missing":[]}}"#)
    check("还没和别的设备同步过", pending.map { notes($0) } == [CostNote(text: "还没和别的设备同步过")])
    check("不认识的同步状态当没同步成；没名字的设备不算；超过三台写「等」", weird?.syncState == .error && weird?.sync?.state.rawValue == "later"
          && weird.map({ notes($0) }) == [CostNote(text: "上次同步没成功", warning: true),
                                  CostNote(text: "还有 4 台设备没有算进来（a、b、c 等，可能 cca 太旧）", warning: true)],
          "\(String(describing: weird.map { notes($0) }))")
    check("关掉了设备统计：说只算了这台设备；cca 没给 sync：当作只有这台设备，但不知道为什么，不说",
          off.map({ notes($0) }) == [CostNote(text: "只算了这台设备：这台设备关掉了设备统计")]
          && AccountCost().syncState == .localOnly && notes(AccountCost()).isEmpty)
    check("还没有记录", Texts.costEmpty(.claude) == "还没有记录：用 Claude Code 干过活以后会显示在这里。"
          && Texts.costEmpty(.codex) == "还没有记录：用 Codex 干过活以后会显示在这里。")
    check("从什么时候开始有记录", Texts.costSince(bob.sinceEpoch!, tool: .claude, now: now, calendar: sh)
          == "从 9 月 10 日开始有记录。"
          && Texts.costSince(bob.sinceEpoch!, tool: .codex, now: now, calendar: sh).hasSuffix("Codex 0.157 起的会话才带着账号信息，更早的算不进来。"))
    check("没有公开价格的模型", Texts.costUnpriced(["claude-research-preview"], requests: 4) == "这些模型没有公开价格，只计了请求数：claude-research-preview（近 12 周 4 次请求）"
          && Texts.costUnpriced(["a", "b"], requests: 0) == "这些模型没有公开价格，只计了请求数：a、b" && Texts.costUnpriced([], requests: 3) == nil)
    check("上次和别的设备同步", Texts.costSynced(1794041700, now: now, calendar: sh) == "上次和别的设备同步：16:55")
    check("设备的叫法", bob.devices.map(Texts.costDeviceName) == ["studio-mac（这台）", "alice-mini", "office-imac", "old-mini（已撤销）"])
    check("多久以前统计的", Texts.costFreshness(CostScan(state: .ready, updatedEpoch: now - 120), now: now) == "2 分钟前统计"
          && Texts.costFreshness(CostScan(state: .ready, updatedEpoch: now - 20), now: now) == "刚刚统计"
          && Texts.costFreshness(CostScan(state: .scanning, updatedEpoch: now - 120), now: now) == "正在统计…"
          && Texts.costFreshness(CostScan(state: .never), now: now) == "还没统计"
          && Texts.costFreshness(CostScan(state: .ready), now: now) == nil && Texts.costFreshness(nil, now: now) == nil)
    let u = s.costTotal?.unattributed, lab = "f0f0f0f0f0f0f0f0"
    check("认不出是哪个账号的（3.6.8 起有所有设备的）：说没算进合计、列在最下面，不说原因；本机、另一台设备时说那一台的，没有的设备不说",
          Texts.costUnattributed(u, scope: .all, total: s.costTotal, currency: "USD")
          == "认不出是哪个账号的用量没算进合计，单独列在最下面。"
          && Texts.costUnattributed(u, currency: "USD") == "这台设备认不出是哪个账号的用量没算进合计，单独列在最下面。"
          && Texts.costUnattributed(u, scope: .device(aliceMini), total: s.costTotal, currency: "USD")
          == "alice-mini 认不出是哪个账号的用量没算进合计，单独列在最下面。"
          && Texts.costUnattributed(u, scope: .device(lab), total: s.costTotal, currency: "USD")
          == "lab-studio 认不出是哪个账号的用量没算进合计，单独列在最下面。"
          && Texts.costUnattributed(u, scope: .device(officeImac), total: s.costTotal, currency: "USD") == nil
          && Texts.costUnattributed(nil, currency: "USD") == nil,
          "\(String(describing: Texts.costUnattributed(u, scope: .all, total: s.costTotal, currency: "USD")))")
    let oldU = try? JSONDecoder().decode(CostUnattributed.self, from: Data(#"{"days_usd":12.3,"requests":40,"why":[],"claude_code":"2.1.330"}"#.utf8))
    check("旧版 cca（只有这台设备近 30 天的）：照旧说金额，看另一台设备时不说", oldU?.cost == nil && oldU?.unsynced == []
          && Texts.costUnattributed(oldU, currency: "USD") == "近 30 天另有 $12.30 认不出是哪个账号的，没算进来。"
          && Texts.costUnattributed(oldU, scope: .all, currency: "USD") == Texts.costUnattributed(oldU, currency: "USD")
          && Texts.costUnattributed(oldU, scope: .device(aliceMini), currency: "USD") == nil)
    check("还没传认不出的用量的设备", Texts.costUnsynced(u, scope: .all) == "还有 2 台设备没传认不出账号的用量（office-imac、old-mini），可能 cca 太旧。"
          && Texts.costUnsynced(u, scope: .me) == nil && Texts.costUnsynced(u, scope: .device(officeImac)) == "office-imac 还没传认不出账号的用量，可能 cca 太旧。"
          && Texts.costUnsynced(u, scope: .device(aliceMini)) == nil && Texts.costUnsynced(oldU, scope: .all) == nil,
          "\(String(describing: Texts.costUnsynced(u, scope: .all)))")
    check("「认不出账号」那一行的名字", Texts.costOrphanRowName(.all, total: s.costTotal) == "所有设备" && Texts.costOrphanRowName(.me, total: s.costTotal) == "这台设备"
          && Texts.costOrphanRowName(.device(aliceMini), total: s.costTotal) == "alice-mini" && Texts.costOrphanRowName(.device(lab), total: s.costTotal) == "lab-studio"
          && Texts.costOrphanRowName(.device("SHA256:x"), total: s.costTotal) == "那台设备")
    check("认不出账号那一组里设备的诊断照样解码（App 不显示原因）",
          u?.cost?.device(lab)?.diag?.why.map(\.code) == [.claudeApp] && u?.cost?.me?.diag?.why.isEmpty != false)
    check("按模型的小标题、叫法", Texts.costModelsTitle(.week) == "按模型 · 近 12 周" && Texts.costModelsTitle(.day) == "按模型 · 近 30 天"
          && Texts.costModelName(CostModelRow(model: nil)) == "没按模型记下的" && Texts.costModelName(CostModelRow(model: "")) == "没写模型名"
          && Texts.costModelName(CostModelRow(model: "claude-opus-5-5")) == "claude-opus-5-5")
    let opus = CostModelRow(model: "claude-opus-5-5", weeks: CostModelRange(usd: 790.1, requests: 8804))
    let unpricedRow = CostModelRow(model: "claude-research-preview", weeks: CostModelRange(requests: 4, unpricedRequests: 4))
    let restRow = CostModelRow(model: nil, weeks: CostModelRange(usd: 22.3, requests: 316, unpricedRequests: 4))
    check("按模型那一行：金额、请求数、读屏、提示（都没有公开价格的写出来；没按模型记下的说为什么）",
          Texts.costModelMoney(opus.weeks, currency: "USD") == "$790" && Texts.costModelRequests(opus.weeks) == "8,804 次"
          && Texts.costModelMoney(unpricedRow.weeks, currency: "USD") == "没有公开价格"
          && Texts.costModelValue(opus, granularity: .week, currency: "USD") == "$790，8,804 次请求"
          && Texts.costModelValue(unpricedRow, granularity: .week, currency: "USD") == "没有公开价格，4 次请求"
          && Texts.costModelValue(restRow, granularity: .week, currency: "USD") == "$22.30，316 次请求，其中 4 次没有公开价格、没算钱"
          && Texts.costModelHelp(restRow, granularity: .week, currency: "USD")
          == "$22.30，316 次请求，其中 4 次没有公开价格、没算钱\n以前的版本统计的、旧版 cca 的设备传来的，只有合计，没有按模型分开"
          && Texts.costModelHelp(opus, granularity: .week, currency: "USD") == Texts.costModelValue(opus, granularity: .week, currency: "USD"),
          "\(Texts.costModelValue(restRow, granularity: .week, currency: "USD")) / \(Texts.costModelRequests(opus.weeks))")
    let hitRow = CostModelRow(model: "claude-opus-5-5", weeks: CostModelRange(usd: 790.1, requests: 8804, cacheHit: 0.9731))
    check("按模型：缓存命中率那一栏（cca 没给时空着）、读屏和悬停", Texts.costModelHit(hitRow.weeks) == "命中 97.3%" && Texts.costModelHit(opus.weeks) == ""
          && Texts.costModelValue(hitRow, granularity: .week, currency: "USD") == "$790，8,804 次请求，缓存命中 97.3%"
          && Texts.costModelHelp(hitRow, granularity: .week, currency: "USD") == Texts.costModelValue(hitRow, granularity: .week, currency: "USD"),
          Texts.costModelValue(hitRow, granularity: .week, currency: "USD"))
    let hitSummary = try? JSONDecoder().decode(CostSummary.self, from: Data(#"{"weeks_cache_hit": 0.9731, "days_cache_hit": null}"#.utf8))
    let clampedHit = try? JSONDecoder().decode(CostSummary.self, from: Data(#"{"weeks_cache_hit": 1.5, "days_cache_hit": -0.2}"#.utf8))
    let oddHit = try? JSONDecoder().decode(CostSummary.self, from: Data(#"{"weeks_cache_hit": "97%", "days_cache_hit": true}"#.utf8))
    let oldHit = try? JSONDecoder().decode(CostSummary.self, from: Data(#"{"weeks_usd": 1}"#.utf8))
    check("13.7.6 起）：数字照收（超出 0～1 的夹回来）；null、别的类型、旧版 cca 没有：nil，不影响别的字段",
          hitSummary?.weeksCacheHit == 0.9731 && hitSummary?.daysCacheHit == nil && clampedHit?.weeksCacheHit == 1 && clampedHit?.daysCacheHit == 0
          && oddHit != nil && oddHit?.weeksCacheHit == nil && oddHit?.daysCacheHit == nil && oldHit?.weeksUSD == 1 && oldHit?.weeksCacheHit == nil)
    let hitRange = try? JSONDecoder().decode(CostModelRange.self, from: Data(#"{"usd": 1, "requests": 2, "cache_hit": 0.5}"#.utf8))
    let oldRange = try? JSONDecoder().decode(CostModelRange.self, from: Data(#"{"usd": 1, "requests": 2}"#.utf8))
    check("按模型的缓存命中率（cache_hit；旧版 cca 没有）", hitRange?.cacheHit == 0.5 && hitRange?.requests == 2 && oldRange != nil && oldRange?.cacheHit == nil)
    // 3.6.14 起：cca 的价格表里还没有的新模型按同系列的标价估算（priced_as）
    let newer = CostModelRow(model: "claude-opus-5-6", weeks: CostModelRange(usd: 12.5, requests: 30), pricedAs: "claude-opus-5-5")
    let newer2 = CostModelRow(model: "gpt-6.3-sol", weeks: CostModelRange(usd: 3, requests: 3), days: CostModelRange(usd: 1, requests: 1),
                              pricedAs: "gpt-6.2-sol")
    check("价格表里还没有的新模型：金额前面加 ≈，读屏、悬停说按哪个模型的价格估算；列表下面说检测到了哪个新模型",
          Texts.costModelMoney(newer.weeks, currency: "USD", estimated: true) == "≈ $12.50"
          && Texts.costModelMoney(unpricedRow.weeks, currency: "USD", estimated: true) == "没有公开价格"
          && Texts.costModelValue(newer, granularity: .week, currency: "USD") == "≈ $12.50，30 次请求，按 claude-opus-5-5 的价格估算"
          && Texts.costModelHelp(newer, granularity: .week, currency: "USD").hasSuffix("cca 收录了它的价格以后自动重算")
          && Texts.costEstimated([opus, newer], granularity: .week) == "检测到新模型 claude-opus-5-6：cca 的价格表里还没有它，按 claude-opus-5-5 的价格估算（≈）"
          && Texts.costEstimated([opus, newer, newer2], granularity: .week) == "检测到新模型 claude-opus-5-6、gpt-6.3-sol：cca 的价格表里还没有，按同系列的价格估算（≈）"
          && Texts.costEstimated([opus, newer, newer2], granularity: .day) == "检测到新模型 gpt-6.3-sol：cca 的价格表里还没有它，按 gpt-6.2-sol 的价格估算（≈）"
          && Texts.costEstimated([opus, restRow], granularity: .week) == nil,
          Texts.costModelValue(newer, granularity: .week, currency: "USD"))
    let pricedRows = try? JSONDecoder().decode([CostModelRow].self, from: Data(#"""
        [{"model": "claude-opus-5-6", "weeks": {"usd": 1, "requests": 1, "unpriced_requests": 0}, "days": {}, "priced_as": "claude-opus-5-5"},
         {"model": "claude-opus-5-5", "weeks": {"usd": 1, "requests": 1, "unpriced_requests": 0}, "days": {}},
         {"model": "x", "weeks": {}, "days": {}, "priced_as": 3}, {"model": "y", "weeks": {}, "days": {}, "priced_as": ""},
         {"model": null, "weeks": {}, "days": {}, "priced_as": "z"}]
        """#.utf8))
    check("priced_as：模型名；没有、空的、类型不对、没按模型记下的那一项都当没有",
          pricedRows?.map(\.pricedAs) == ["claude-opus-5-5", nil, nil, nil, nil], "\(String(describing: pricedRows?.map(\.pricedAs)))")
    let mrows = [CostModelRow(model: "a", weeks: CostModelRange(usd: 9, requests: 9), days: CostModelRange(usd: 1, requests: 1)),
                 CostModelRow(model: "b", weeks: CostModelRange(usd: 5, requests: 5), days: CostModelRange(usd: 4, requests: 4)),
                 CostModelRow(model: "d", weeks: CostModelRange(usd: 2, requests: 2)),
                 CostModelRow(model: "e", weeks: CostModelRange(usd: 0.5, requests: 5), days: CostModelRange(usd: 1, requests: 5)),
                 CostModelRow(model: "c", weeks: CostModelRange(requests: 3, unpricedRequests: 3), days: CostModelRange(requests: 2, unpricedRequests: 2)),
                 CostModelRow(model: nil, weeks: CostModelRange(usd: 1, requests: 1), days: CostModelRange(usd: 7, requests: 7))]
    check("按模型列哪几行：按周照 cca 的顺序；按天按 30 天的金额、请求数重新排，没按模型记下的照旧在最后；这一段没有用量的不列",
          CostChart.modelRows(mrows, granularity: .week).map(\.id) == ["m:a", "m:b", "m:d", "m:e", "m:c", "rest"]
          && CostChart.modelRows(mrows, granularity: .day).map(\.id) == ["m:b", "m:e", "m:a", "m:c", "rest"],
          CostChart.modelRows(mrows, granularity: .day).map(\.id).joined(separator: ","))
    let weekRows = CostChart.modelRows(mrows, granularity: .week)
    check("按模型那一行的条：金额占这几行加起来的多少；都没有金额时是 0",
          abs(CostChart.modelShare(mrows[0], of: weekRows, granularity: .week) - 9 / 17.5) < 1e-9
          && CostChart.modelShare(mrows[4], of: weekRows, granularity: .week) == 0
          && CostChart.modelShare(unpricedRow, of: [unpricedRow], granularity: .week) == 0)
    let withWhy = CostDevice(id: "x", name: "x", isMe: false, revoked: false, syncedEpoch: nil, stale: false, weeksUSD: [], daysUSD: [],
                             summary: CostSummary(), diag: CostDiag(claudeCodeOld: true, why: [CostWhy(code: .claudeApp, lastEpoch: 1)]))
    check("设备有认不出账号的回复、命令行里的 Claude Code 太旧（cca 给的诊断）：都不说",
          Texts.costNotes(AccountCost(devices: [withWhy]), now: 1, calendar: sh).isEmpty
          && Texts.costNotes(AccountCost(devices: [withWhy]), scope: .device("x"), now: 1, calendar: sh).isEmpty)
    let onlyRequests = try? JSONDecoder().decode(CostUnattributed.self, from: Data(#"{"days_usd":0,"requests":5}"#.utf8))
    let nothing = try? JSONDecoder().decode(CostUnattributed.self, from: Data(#"{"days_usd":0,"requests":0}"#.utf8))
    check("认不出的只有请求数时说请求数，都没有时不说", Texts.costUnattributed(onlyRequests, currency: "USD")?.hasPrefix("近 30 天另有 5 次请求认不出") == true
          && Texts.costUnattributed(nothing, currency: "USD") == nil)

    section("折合金额 · 解码")
    let messy = decodeCost(#"""
    {"currency":"USD","prices_date":"2026-11-01","since_epoch":"x",
     "weeks":[{"start_epoch":200,"end_epoch":300,"usd":"2.5","current":true},
              {"start_epoch":100,"usd":1},
              {"start_epoch":0,"end_epoch":100,"usd":-4,"requests":3,"unpriced_requests":"1","basis":"lunar","tokens":{"input":5,"output":"x"}},
              "junk"],
     "days":[{"start_epoch":0,"end_epoch":86400,"date":"","usd":1}],
     "summary":{"weeks_usd":"3.5","average_usd":null,"average_weeks":2.0},
     "devices":[{"id":"","name":"mini","weeks_usd":[1,2,3,4],"days_usd":[null]},
                {"me":true},
                {"id":"SHA256:x","weeks_usd":["0.5",null,"bad",9],"days_usd":[]}],
     "unpriced_models":["", "m", 3],
     "sync":{"state":7,"missing":[{"name":"a"},{"used_epoch":5}]}}
    """#)
    let diags = decodeCost(#"""
    {"weeks":[],"days":[],
     "devices":[{"id":"a","diag":{"cca":"","claude_code":3,"claude_code_old":"yes",
                                  "why":[{"code":"old_claude_code","last_epoch":"1793923200","claude_code":"2.1.316"},{"code":"","last_epoch":1},
                                         {"code":"x"},{"code":"vscode","last_epoch":2,"claude_code":""},"junk",null]}},
                {"id":"b","diag":"x"},{"id":"c","diag":null},{"id":"d","diag":{}}]}
    """#)
    check("坏掉的周期不要，剩下的按时间排好", messy?.weeks.map(\.startEpoch) == [0, 200] && messy?.weeks.last?.current == true)
    check("金额写成字符串也认，负数当 0", messy?.weeks.map(\.usd) == [0, 2.5] && messy?.weeks.first?.requests == 3 && messy?.weeks.first?.unpricedRequests == 1)
    check("不认识的 basis 原样留着", messy?.weeks.first?.basis?.rawValue == "lunar" && messy?.weeks.first?.basis?.isKnown == false)
    check("token 里坏掉的当 0", messy?.weeks.first?.tokens == CostTokens(input: 5))
    check("空的日期当没有", messy?.days.first?.date == nil)
    check("设备的数组跟着周期一起排好、去掉坏掉的那一项（缺的、坏的是 0）",
          messy?.devices.first?.weeksUSD == [3, 1] && messy?.devices.first?.daysUSD == [0]
          && messy?.devices.last?.weeksUSD == [0, 0.5] && messy?.devices.last?.daysUSD == [0], "\(String(describing: messy?.devices))")
    check("没有 id 的设备用名字，id、名字都没有的不要；名字缺了用 id", messy?.devices.map(\.id) == ["mini", "SHA256:x"]
          && messy?.devices.last?.name == "SHA256:x")
    check("summary 宽容", messy?.summary.weeksUSD == 3.5 && messy?.summary.averageUSD == nil && messy?.summary.averageWeeks == 2
          && messy?.summary.averageDayUSD == nil && messy?.summary.averageDays == 0)
    check("since_epoch 坏了当没有", messy?.sinceEpoch == nil)
    check("没有公开价格的模型：只要非空字符串", messy?.unpricedModels == ["m"])
    check("同步状态不是字符串：当没同步成；没名字的设备不算", messy?.syncState == .error && messy?.sync?.missing.map(\.name) == ["a"])
    check("设备的诊断宽容：坏掉的原因跳过（没有 code、last_epoch 的），不认识的原因留着当别的说；不是对象的当没有",
          diags?.devices.map(\.diag) == [CostDiag(why: [CostWhy(code: .oldClaudeCode, lastEpoch: 1793923200, claudeCode: "2.1.316"),
                                                           CostWhy(code: CostWhyCode(rawValue: "vscode"), lastEpoch: 2)]),
                                          nil, nil, CostDiag()]
          && diags?.devices.first?.diag?.why.last?.code.shown == .unknown, "\(String(describing: diags?.devices.map(\.diag)))")
    check("没有数组的周期：空的", decodeCost(#"{}"#).map { $0.weeks.isEmpty && $0.days.isEmpty && $0.devices.isEmpty && $0.currency == "USD" } == true)
    let total = try? JSONDecoder().decode(CostTotal.self, from: Data(#"""
    {"current_usd":"2","average_usd":null,"days":[{"start_epoch":86400,"end_epoch":172800,"usd":1,"current":true},{"start_epoch":0,"end_epoch":86400,"usd":2}],
     "by_tool":{"claude":{"current_usd":2},"gemini":{"current_usd":5},"codex":"x"},"scan":{"state":"warming"}}
    """#.utf8))
    check("cost_total：顶层的几项、天按时间排好", total?.all.summary.currentUSD == 2 && total?.all.summary.averageUSD == nil
          && total?.all.days.map(\.startEpoch) == [0, 86400] && total?.all.today?.startEpoch == 86400 && total?.currency == "USD")
    check("cost_total：by_tool 只认认识的工具、坏掉的不要", total?.all.byTool.keys.map(\.rawValue).sorted() == ["claude"] && total?.all.byTool[.claude]?.currentUSD == 2)
    check("cost_total：旧版 cca 没有 me、average_day_usd：本机没有，平均每天是 nil", total?.me == nil && total?.part(.me) == nil
          && total?.all.summary.averageDayUSD == nil && total?.part(.all) == total?.all)
    let withMe = try? JSONDecoder().decode(CostTotal.self, from: Data(#"""
    {"current_usd":3,"average_day_usd":"1.5","me":{"current_usd":1,"average_day_usd":0.5,"days":[{"start_epoch":0,"end_epoch":86400,"usd":1,"current":true}],
     "by_tool":{"codex":{"today_usd":1}}},"days":[]}
    """#.utf8))
    check("cost_total.me：和顶层一样解码（几项、天、按工具）", withMe?.all.summary.averageDayUSD == 1.5 && withMe?.me?.summary.currentUSD == 1
          && withMe?.me?.summary.averageDayUSD == 0.5 && withMe?.me?.today?.usd == 1 && withMe?.me?.byTool[.codex]?.todayUSD == 1
          && withMe?.part(.me) == withMe?.me)
    check("cost_total：旧版 cca 没有 devices：空的（菜单里只有所有设备、本机），另一台设备看不了", total?.devices.isEmpty == true
          && total?.otherDevices.isEmpty == true && total?.part(.device("x")) == nil)
    let withDevices = try? JSONDecoder().decode(CostTotal.self, from: Data(#"""
    {"current_usd":3,"devices":[{"id":"m","name":"mac","me":true,"current_usd":1,"days":[]},
                                {"id":"o","revoked":true,"stale":1,"current_usd":"2","by_tool":{"claude":{"current_usd":2}}},
                                {"name":"no-id"},{"id":""},"junk",
                                {"id":"n","name":"new","stale":true}]}
    """#.utf8))
    check("cost_total.devices：和 me 一样解码几项，加上 id、名字（缺了用 id）；没有 id 的不要，坏掉的标记当 false",
          withDevices?.devices.map(\.id) == ["m", "o", "n"] && withDevices?.devices.map(\.name) == ["mac", "o", "new"]
          && withDevices?.devices.map(\.isMe) == [true, false, false] && withDevices?.devices.map(\.stale) == [false, false, true]
          && withDevices?.devices[1].revoked == true
          && withDevices?.devices[1].part.summary.currentUSD == 2 && withDevices?.devices[1].part.byTool[.claude]?.currentUSD == 2
          && withDevices?.part(.device("o")) == withDevices?.devices[1].part && withDevices?.otherDevices.map(\.id) == ["o", "n"],
          "\(String(describing: withDevices?.devices))")
    let modelled = decodeCost(#"{"weeks":[],"days":[],"models":[{"model":"a","weeks":{"usd":"x","requests":-3,"unpriced_requests":2},"#
                              + #""days":{"usd":1.5,"requests":2,"tokens":{"input":1,"output":2,"cache_read":3,"cache_write":4}}},"#
                              + #"{"model":null},{"weeks":{}},{"model":3},{"model":""}],"devices":[{"id":"d","models":"bad"},{"id":"e","models":[{"model":"a"}]}]}"#)
    check("按模型：宽容解码（model 缺了、不是字符串的那一项不要；数字坏了当 0；设备的坏了当没有）",
          modelled?.models?.map(\.id) == ["m:a", "rest", "m:"] && modelled?.models?.first?.weeks == CostModelRange(unpricedRequests: 2)
          && modelled?.models?.first?.days == CostModelRange(usd: 1.5, requests: 2, tokens: CostTokens(input: 1, output: 2, cacheRead: 3, cacheWrite: 4))
          && modelled?.devices.map { $0.models?.count } == [nil, 1], "\(String(describing: modelled?.models))")
    check("旧版 cca 没有按模型的：nil（不显示那一块）", decodeCost(#"{"weeks":[],"days":[]}"#)?.models == nil
          && (try? JSONDecoder().decode(CostTotal.self, from: Data(#"{"me":{}}"#.utf8)))?.all.models == nil)
    let unsyncedDoc = try? JSONDecoder().decode(CostUnattributed.self, from: Data(
        #"{"days_usd":0,"requests":0,"unsynced":[{"id":"a","name":"x"},{"id":""},{"name":"y"},{"id":"b"}],"cost":{"weeks":[],"days":[]}}"#.utf8))
    check("认不出的：unsynced 没有 id 的不要、没名字用 id；cost 照账号的解码", unsyncedDoc?.unsynced.map(\.id) == ["a", "b"]
          && unsyncedDoc?.unsynced.map(\.name) == ["x", "b"] && unsyncedDoc?.cost != nil)
    let used = decodeCost(#"{"weeks":[],"days":[],"devices":[{"id":"a"},{"id":"b","used":false},{"id":"c","used":"no"}]}"#)
    check("账号里的设备：used 缺了、坏了当用过（旧版 cca）", used?.devices.map(\.used) == [true, false, true])
    let badMe = try? JSONDecoder().decode(CostTotal.self, from: Data(#"{"current_usd":3,"me":"x"}"#.utf8))
    check("cost_total.me 坏了：当没有（顶层照用）", badMe?.me == nil && badMe?.all.summary.currentUSD == 3)
    check("cost_total：不认识的统计状态原样留着（App 当统计过）", total?.scan.state.rawValue == "warming"
          && total?.scan.state.isKnown == false && total?.unattributed == nil)
    let noScan = try? JSONDecoder().decode(CostTotal.self, from: Data(#"{}"#.utf8))
    check("cost_total：没有 scan 当统计过", noScan?.scan.state == .ready && noScan?.all.byTool.isEmpty == true)
    let sync = try? DocumentDecoder.decode(CostSyncDoc.self, from: Data(#"{"version":1,"kind":"cost_sync","scan":{"state":"ready","files":"x"},"sync":{"state":"rate_limited"}}"#.utf8))
    check("cost sync：宽容解码，不认识的同步状态原样留着", sync?.scan?.files == 0 && sync?.sync?.state.rawValue == "rate_limited" && sync?.sync?.state.isKnown == false)
    check("cost sync：kind 不对的不认", (try? DocumentDecoder.decode(CostSyncDoc.self, from: Data(#"{"version":1,"kind":"usage"}"#.utf8))) == nil)

    section("折合金额 · 命令和节奏")
    let auto = CcaCommand.costSync(manual: false), manual = CcaCommand.costSync(manual: true)
    check("命令：cca cost sync --json，不带工具前缀；定时的低优先级", auto.arguments == ["cost", "sync", "--json"] && auto.tool == nil && auto.key == nil
          && auto.isBackground && auto.timeout == 300 && !auto.runsToCompletion)
    check("点「刷新」：带 --now，不降优先级", manual.arguments == ["cost", "sync", "--now", "--json"] && !manual.isBackground && manual.timeout == 300)
    check("一次一个（定时的和手动的排队），不和别的命令抢", auto.gate == manual.gate && auto.gate == "cost-sync"
          && auto.gate != CcaCommand.usage(.claude, key: nil).gate && auto.gate != CcaCommand.routerSync.gate)
    let t0 = 1_800_000_000.0
    check("还没统计过：马上统计", CostPolling.isDue(lastAttempt: nil, updated: nil, never: true, visible: false, now: t0))
    check("还没统计过、刚试过没成：按间隔重试", !CostPolling.isDue(lastAttempt: t0 - 60, updated: nil, never: true, visible: true, now: t0)
          && CostPolling.isDue(lastAttempt: t0 - 301, updated: nil, never: true, visible: true, now: t0))
    check("看得见：5 分钟一次", !CostPolling.isDue(lastAttempt: nil, updated: t0 - 299, never: false, visible: true, now: t0)
          && CostPolling.isDue(lastAttempt: nil, updated: t0 - 301, never: false, visible: true, now: t0))
    check("看不见：30 分钟一次", !CostPolling.isDue(lastAttempt: nil, updated: t0 - 1799, never: false, visible: false, now: t0)
          && CostPolling.isDue(lastAttempt: nil, updated: t0 - 1801, never: false, visible: false, now: t0))
    check("cca 自己统计过（会话结束时）也算：取晚的那个", !CostPolling.isDue(lastAttempt: t0 - 4000, updated: t0 - 100, never: false, visible: false, now: t0)
          && !CostPolling.isDue(lastAttempt: t0 - 100, updated: t0 - 4000, never: false, visible: false, now: t0)
          && CostPolling.isDue(lastAttempt: nil, updated: nil, never: false, visible: false, now: t0))
    check("间隔", CostPolling.interval(visible: true) == 300 && CostPolling.interval(visible: false) == 1800)

    let homes = ["/Users/me/.cca"]
    let costCases: [(String, CcaWatcher.Trigger?)] = [
        ("/Users/me/.cca/cost/local.json", .state),
        // 不带 Team 编号的 remote.json、sync.json 是维护者的服务器（3.6.0 去掉了）留下的；各个 Team 的见 hubFixtureTests
        ("/Users/me/.cca/cost/remote.json", nil), ("/Users/me/.cca/cost/sync.json", nil),
        ("/Users/me/.cca/cost/scan.json", nil), ("/Users/me/.cca/cost/local.json.tmp", nil), ("/Users/me/.cca/cost/.local.json.x1", nil),
        ("/Users/me/.cca/cost/cost.lock", nil), ("/Users/me/.cca/cost", nil), ("/Users/me/.cca/cost/old/local.json", nil),
        ("/Users/me/.cca/cost/usage.json", nil), ("/Users/me/.cca/accounts/a/cost/local.json", nil),
    ]
    let wrong = costCases.filter { CcaWatcher.classify(path: $0.0, homes: homes) != $0.1 }.map(\.0)
    check("看着 cost/ 里的 local.json、各个 Team 的 remote.json、sync.json（别的不算）", wrong.isEmpty, wrong.joined(separator: " "))

    // 演示模式：时间平移时，每一天的本地日期跟着挪整天（最后一天还是“今天”）
    let raw = Data(#"{"generated_at":1794042000,"x_epoch":5,"cost":{"prices_date":"2026-11-01","days":[{"date":"2026-10-31"},{"date":"2026-11-07"},{"date":"x"}]}}"#.utf8)
    let moved = jsonObject(FixtureTransport.rebase(raw, by: 3 * 86400, days: 3)) as? [String: Any]
    let mc = moved?["cost"] as? [String: Any], mdays = (mc?["days"] as? [[String: Any]])?.map { $0["date"] as? String }
    check("演示模式：日期挪整天（跨月），价格表的日期、认不出的不动", mdays == ["2026-11-03", "2026-11-10", "x"] && mc?["prices_date"] as? String == "2026-11-01"
          && (moved?["x_epoch"] as? NSNumber)?.doubleValue == 5 + 3 * 86400, "\(String(describing: mdays))")
    let back = jsonObject(FixtureTransport.rebase(raw, by: 0, days: -7)) as? [String: Any]
    check("演示模式：只挪日期也行（往回挪）", ((back?["cost"] as? [String: Any])?["days"] as? [[String: Any]])?.first?["date"] as? String == "2026-10-24")
    check("不平移：原样", FixtureTransport.rebase(raw, by: 0, days: 0) == raw)
    check("隔了几个本地日历日（按零点数）", FixtureTransport.days(from: generatedAt, to: generatedAt + 6 * 3600, calendar: sh) == 0
          && FixtureTransport.days(from: generatedAt, to: generatedAt + 7 * 3600, calendar: sh) == 1
          && FixtureTransport.days(from: generatedAt, to: generatedAt - 18 * 3600, calendar: sh) == -1)
    let demo = FixtureTransport(directory: fixtureDir, stepDelay: 0, rebaseTo: generatedAt + 10 * 86400 + 8 * 3600, calendar: sh)
    check("演示模式：平移以后那天", demo.dayShift == 11 && demo.shift == 10 * 86400 + 8 * 3600, "\(demo.dayShift)")
}

// MARK: - AppModel

/// cost sync 怎么回：成功、没统计成（cost_unavailable）、关掉了（cost_off）
private final class CostReply: @unchecked Sendable {
    /// hold：`cost sync` 一直不回，直到测试 `gate.signal()`（看统计途中的状态）
    enum Mode { case ok, fail(String), off, hold }
    let gate = DispatchSemaphore(value: 0)
    private let lock = NSLock()
    private var _mode = Mode.ok
    var mode: Mode {
        get { lock.withLock { _mode } }
        set { lock.withLock { _mode = newValue } }
    }
}

/// state.json 改一改（cost_total 的统计状态、整个去掉）
private func costState(_ edit: (inout [String: Any]) -> Void) -> String {
    var root = (fixture("state.json").flatMap { try? JSONSerialization.jsonObject(with: $0) }) as? [String: Any] ?? [:]
    edit(&root)
    return String(decoding: (try? JSONSerialization.data(withJSONObject: root)) ?? Data(), as: UTF8.self)
}

@MainActor func costModelTests() async {
    section("折合金额 · AppModel")
    guard let helloText = fixture("hello.json").map({ String(decoding: $0, as: UTF8.self) }),
          let stateText = fixture("state.json").map({ String(decoding: $0, as: UTF8.self) }),
          let syncText = fixture("cost_sync.json").map({ String(decoding: $0, as: UTF8.self) }),
          let oldHello = fixture("hello_nocodex_notinstalled.json").map({ String(decoding: $0, as: UTF8.self) }) else {
        check("夹具", false)
        return
    }
    func model(hello: String = helloText, state: String = stateText, reply: CostReply = CostReply(), log: HookLog? = nil,
               prefs: MemoryPrefs = MemoryPrefs(), delay: TimeInterval = 0) -> (ScriptedTransport, AppModel) {
        let t = ScriptedTransport(delay: delay) { request, _ in
            if request.arguments == ["_gui", "hello"] { return response(0, stdout: hello) }
            if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
            if request.arguments.first == "cost" {
                switch reply.mode {
                case .ok: return response(0, stdout: syncText)
                case .fail(let m): return response(1, [.fail(FailEvent(code: .costUnavailable, message: m))])
                case .off: return response(2, [.fail(FailEvent(code: .costOff, message: "折合金额关掉了"))])
                case .hold:
                    reply.gate.wait()
                    return response(0, stdout: syncText)
                }
            }
            return response(0, stdout: SampleDocs.clientUpdates)
        }
        return (t, makeModel(t, log: log ?? HookLog(), prefs: prefs))
    }
    func syncs(_ t: ScriptedTransport) -> [CcaRequest] { t.requests.filter { $0.arguments.first == "cost" } }
    func states(_ t: ScriptedTransport) -> Int { t.requests.filter { $0.arguments == ["_gui", "state"] }.count }

    // 旧版 cca：hello 里没有 cost，不统计、不显示
    do {
        let (t, m) = model(hello: oldHello)
        await m.start()
        m.setMainVisible(true)
        m.selection = .costStats
        m.refreshUsageIfNeeded()
        m.refreshCost()
        try? await Task.sleep(nanoseconds: 100_000_000)
        check("旧版 cca（hello 没有 cost）：不显示折合金额、不统计", !m.costAvailable && !m.costEnabled && !m.costVisible && syncs(t).isEmpty)
        m.shutdown()
    }
    // hello 的版本不认识：一样当作没有（侧栏不列折合金额页，选中了也在读到 state 时回总览）
    do {
        let (t, m) = model(hello: helloText.replacingOccurrences(of: #""cost": 1"#, with: #""cost": 2"#))
        await m.start()
        m.refreshCost()
        try? await Task.sleep(nanoseconds: 50_000_000)
        check("hello.cost 不是认识的版本：当作没有", !m.costAvailable && syncs(t).isEmpty)
        m.selection = .costStats
        let before = states(t)
        m.refreshState()
        check("没有认识的折合金额：读到 state 时从折合金额页回总览", await waitUntil { states(t) > before && m.selection == .office },
              "\(String(describing: m.selection))")
        m.shutdown()
    }
    // cost_total 是 null（关掉了、没有可统计的账号）：不统计
    do {
        let (t, m) = model(state: costState { $0["cost_total"] = NSNull() })
        await m.start()
        m.setMainVisible(true)
        m.selection = .costStats
        m.refreshUsageIfNeeded()
        m.refreshCost()
        try? await Task.sleep(nanoseconds: 100_000_000)
        check("cost_total 为 null：不显示、不统计", m.costAvailable && !m.costEnabled && !m.costVisible && syncs(t).isEmpty)
        let before = states(t)
        m.refreshState()
        check("cost_total 为 null：折合金额页照样留着（页面上说为什么没有数字）", await waitUntil { states(t) > before }
              && m.selection == .costStats)
        m.shutdown()
    }
    // 还没统计过（scan.state 是 never）：读到 state 就马上统计一次（看不见也是），统计完重读 state
    do {
        let never = costState {
            var total = $0["cost_total"] as? [String: Any] ?? [:]
            total["scan"] = ["state": "never", "updated_epoch": NSNull()] as [String: Any]
            $0["cost_total"] = total
        }
        let held = CostReply()
        held.mode = .hold
        let (t, m) = model(state: never, reply: held)
        await m.start()
        check("还没统计过：马上统计一次（低优先级）", await waitUntil { syncs(t).count == 1 } && syncs(t).first?.background == true
              && syncs(t).first?.arguments == ["cost", "sync", "--json"], "\(syncs(t).map(\.arguments))")
        check("跑着的时候算正在统计（转圈、说第一次要全读一遍）", m.costSyncing && m.costScanning)
        held.mode = .ok
        held.gate.signal()
        check("统计完重读 state", await waitUntil { !m.costSyncing && states(t) >= 2 })
        check("cca 还说 never、App 也没在跑：不算正在统计（显示“还没统计”，不一直转圈）", !m.costScanning)
        m.refreshUsageIfNeeded()
        try? await Task.sleep(nanoseconds: 80_000_000)
        check("刚统计过：不再马上统计（cca 还说 never 也按间隔）", syncs(t).count == 1, "\(syncs(t).count)")
        m.shutdown()
    }

    // cca 10 分钟前统计过（会话结束时 cca 自己也会统计）：看不见金额时 30 分钟一次，还不到；看得见时 5 分钟一次，到了
    let fresh = costState {
        var total = $0["cost_total"] as? [String: Any] ?? [:]
        total["scan"] = ["state": "ready", "updated_epoch": Date().timeIntervalSince1970 - 600] as [String: Any]
        $0["cost_total"] = total
    }
    let reply = CostReply()
    let log = HookLog()
    let prefs = MemoryPrefs()
    let (t, m) = model(state: fresh, reply: reply, log: log, prefs: prefs)
    await m.start()
    check("有折合金额：hello.cost 是 1、cost_total 不是 null", m.costAvailable && m.costEnabled)
    m.selection = .office
    m.refreshUsageIfNeeded()
    m.setMainVisible(true)
    try? await Task.sleep(nanoseconds: 80_000_000)
    check("看不见金额、10 分钟前统计过：不统计（看不见时 30 分钟一次）", !m.costVisible && syncs(t).isEmpty, "\(syncs(t).map(\.arguments))")
    m.selection = .usageStats
    try? await Task.sleep(nanoseconds: 50_000_000)
    check("用量统计页：看不见金额，不统计", !m.costVisible && syncs(t).isEmpty)
    m.selection = .costStats
    let visibleNow = m.costVisible
    check("打开折合金额页：看得见（5 分钟一次），马上统计一次（低优先级）", await waitUntil { syncs(t).count == 1 } && visibleNow
          && syncs(t).last?.background == true && syncs(t).last?.arguments == ["cost", "sync", "--json"])
    check("统计完重读 state，还在折合金额页", await waitUntil { !m.costSyncing } && m.selection == .costStats)
    m.refreshUsageIfNeeded()
    m.selection = .account(bobRef)
    try? await Task.sleep(nanoseconds: 80_000_000)
    check("刚统计过：每分钟那一下、换到账号页都不再统计", syncs(t).count == 1, "\(syncs(t).count)")
    m.selection = .account(bobRef)
    check("有「折合金额」一节的账号页：看得见", m.costVisible)
    m.selection = .account(AccountRef(tool: .claude, key: "直连接口"))
    check("第三方 API 账号的账号页：看不见", !m.costVisible)
    m.selection = .office
    check("总览：看不见", !m.costVisible)
    m.setMainVisible(false)
    m.selection = .costStats
    check("窗口看不见：看不见", !m.costVisible)
    m.setMainVisible(true)

    // 点「刷新」：带 --now，不降优先级；转圈；没成就说原因，成了就去掉
    let manualBefore = syncs(t).count
    m.refreshCost()
    let spinning = m.costRefreshing && m.costSyncing
    check("点「刷新」：马上统计（--now，不降优先级）", await waitUntil { syncs(t).count == manualBefore + 1 } && spinning
          && syncs(t).last?.arguments == ["cost", "sync", "--now", "--json"] && syncs(t).last?.background == false)
    check("跑完了不转圈", await waitUntil { !m.costRefreshing && !m.costSyncing } && m.costFailure == nil)
    reply.mode = .fail("另一个统计一直没结束")
    m.refreshCost()
    check("没统计成：说 cca 给的原因，上次的数字照样显示", await waitUntil { m.costFailure == "另一个统计一直没结束" }
          && m.account(bobRef)?.cost != nil, "\(String(describing: m.costFailure))")
    reply.mode = .ok
    m.refreshCost()
    check("又统计成了：原因去掉", await waitUntil { m.costFailure == nil && !m.costRefreshing })
    reply.mode = .fail("x")
    m.refreshCost()
    _ = await waitUntil { m.costFailure != nil && !m.costRefreshing }
    reply.mode = .off
    m.refreshCost()
    check("关掉了（cost_off）：不当作出错", await waitUntil { m.costFailure == nil && !m.costRefreshing })
    reply.mode = .ok

    // 看谁的（所有设备、本机、另一台设备）、按周还是按天：账号页和折合金额页共用一个设置；折合金额页点一行：打开账号页、滚到「折合金额」
    check("默认看所有设备、按周", m.costScope == .all && m.costGranularity == .week)
    m.costScope = .me
    check("换成本机：记在设置里", m.costScope == .me && prefs.string(forKey: "cost.scope") == "me")
    m.selection = .costStats
    m.showCost(bobRef)
    check("折合金额页点一行：打开账号页、滚到折合金额，看的还是本机", m.selection == .account(bobRef) && m.costFocus == bobRef && m.costScope == .me)
    m.costFocus = nil
    let frankRef = AccountRef(tool: .codex, key: "frank@example.com")
    m.showCost(frankRef)
    check("ChatGPT 账号：换到 Codex", m.tool == .codex && m.selection == .account(frankRef))
    m.tool = .claude
    let groups = m.costGroups()
    check("折合金额页的分组：Claude、ChatGPT 的官方账号（第三方 API 账号不列），和总览一样的顺序",
          groups.map(\.department) == [.claude, .chatgpt]
          && groups.first?.accounts.map(\.key) == ["工作(bob@example.com)", "carol@example.com", "测试(dave@example.com)", "erin@example.com", "grace@example.com"]
          && groups.last?.accounts.map(\.key) == ["frank@example.com"], "\(groups.map { $0.accounts.map(\.key) })")
    m.costGranularity = .day
    check("按周 / 按天记在设置里", prefs.string(forKey: "cost.period") == "day")
    m.shutdown()
    let (_, again) = model(prefs: prefs)
    check("下次打开还是按天、本机", again.costGranularity == .day && again.costScope == .me)
    prefs.set("everyone", forKey: "cost.scope")
    let (_, odd) = model(prefs: prefs)
    check("设置里的值不认识：看所有设备", odd.costScope == .all)

    // 另一台设备：设置里记 d:设备 id；折合金额页只列它用过的账号；这次 cca 没列它时看所有设备（设置不改）
    let (tDev, dev) = model(prefs: prefs)
    await dev.start()
    _ = await waitUntil { dev.state != nil }
    dev.costScope = .device(aliceMini)
    check("选另一台设备：记在设置里", prefs.string(forKey: "cost.scope") == "d:" + aliceMini && dev.costScopeShown == .device(aliceMini))
    let carolRef = AccountRef(tool: .claude, key: "carol@example.com")
    check("账号页：选的那台设备用过这个账号时看它；没用过（carol）、没有折合金额时这一页看所有设备，设置不改",
          dev.costScopeShown(for: dev.account(bobRef)?.cost) == .device(aliceMini)
          && dev.account(carolRef)?.cost != nil && dev.costScopeShown(for: dev.account(carolRef)?.cost) == .all
          && dev.costScopeShown(for: nil) == .all && prefs.string(forKey: "cost.scope") == "d:" + aliceMini)
    let devGroups = dev.costGroups()
    check("看另一台设备：折合金额页只列它用过的账号（alice-mini 只用过 bob），没有账号的组不列",
          devGroups.map(\.department) == [.claude] && devGroups.first?.accounts.map(\.key) == ["工作(bob@example.com)"], "\(devGroups.map { $0.accounts.map(\.key) })")
    dev.costScope = .device("SHA256:gone")
    check("选的设备这次 cca 没列出来：看所有设备，设置不改", dev.costScopeShown == .all && prefs.string(forKey: "cost.scope") == "d:SHA256:gone"
          && dev.costGroups().flatMap(\.accounts).count == groups.flatMap(\.accounts).count)
    dev.costScope = .device(me)
    check("选的设备 id 是这台设备的：就是本机（哪个账号页都是）", dev.costScopeShown == .me && dev.costScopeShown(for: dev.account(carolRef)?.cost) == .me)
    dev.shutdown()
    _ = tDev
    let (_, again2) = model(prefs: prefs)
    check("下次打开还是那台设备（d:设备 id）", again2.costScope == .device(me))
    prefs.set("all", forKey: "cost.scope")

    // 用户走开了、过热：不统计
    do {
        let idle = HookLog()
        idle.conditions.idleSeconds = UsagePolling.awayAfter + 1
        let (t2, m2) = model(log: idle)
        await m2.start()
        m2.setMainVisible(true)
        m2.selection = .costStats
        try? await Task.sleep(nanoseconds: 80_000_000)
        check("用户走开了：不统计", syncs(t2).isEmpty)
        idle.conditions = SystemConditions(thermalPressure: true)
        m2.selection = .account(bobRef)
        try? await Task.sleep(nanoseconds: 80_000_000)
        check("系统过热：不统计", syncs(t2).isEmpty)
        idle.conditions = SystemConditions()
        m2.refreshCost()
        check("用户点的「刷新」照样统计", await waitUntil { syncs(t2).count == 1 })
        m2.shutdown()
    }
    // 定时统计排队：一次一个
    do {
        let (t3, m3) = model(delay: 0.2)
        await m3.start()
        m3.setMainVisible(true)
        m3.selection = .costStats
        m3.refreshCost()
        m3.refreshCost()
        _ = await waitUntil { syncs(t3).count >= 2 && !m3.costSyncing }
        let runs = t3.log.filter { $0.label.hasPrefix("cost sync") }
        check("自动的和点的「刷新」排着队（一次一个），连点只算一次", runs.count == 2
              && zip(runs, runs.dropFirst()).allSatisfy { $0.end <= $1.start + 0.001 }, "\(runs.map { ($0.label, $0.start, $0.end) })")
        m3.shutdown()
    }

    // 演示模式：cost sync 回 cost_sync.json
    let demo = makeModel(fixtureTransport())
    await demo.start()
    demo.refreshCost()
    check("演示模式也能刷新", await waitUntil { !demo.costRefreshing } && demo.costFailure == nil && demo.costEnabled)
    demo.shutdown()
}
