import Foundation

// 折合金额（PROTOCOL 2.2 的 `cost`、`cost_total`，3.5 的 `cca cost sync --json`）：手写数据、图怎么排（CostChart）、
// 文字（金额、token、日期、提示）、解码的宽容、看哪些文件、什么时候统计（CostPolling），和 AppModel 里的定时统计、手动刷新。

private let bobRef = AccountRef(tool: .claude, key: "工作(bob@example.com)")
private let generatedAt = 1794042000.0   // 夹具的 generated_at：2026-11-07 周六 17:00（+08）
private let me = "SHA256:" + String(repeating: "a", count: 43)
private let aliceMini = "SHA256:" + String(repeating: "b", count: 43)

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
                                                     todayUSD: 44.06, requests: 36213, unpricedRequests: 4), "\(String(describing: bob?.summary))")
    check("bob：本周的 token", bob?.currentWeek?.tokens == CostTokens(input: 146741, output: 1128071, cacheRead: 199181482, cacheWrite: 5216937))
    check("bob：四台设备，这台在最前，撤销了的排最后", bob?.devices.map(\.name) == ["studio-mac", "alice-mini", "office-imac", "old-mini"]
          && bob?.devices.first?.isMe == true && bob?.devices.first?.id == me)
    check("bob：office-imac 很久没同步上来，old-mini 撤销了", bob?.device("SHA256:" + String(repeating: "c", count: 43))?.stale == true
          && bob?.devices.last?.revoked == true && bob?.devices.last?.stale == true)
    check("bob：每台设备的数组和 weeks、days 一样长", bob?.devices.allSatisfy { $0.weeksUSD.count == 13 && $0.daysUSD.count == 30 } == true)
    check("bob：alice-mini 本周、今天", bob?.device(aliceMini)?.weeksUSD.last == 172.76 && bob?.device(aliceMini)?.daysUSD.last == 14.36
          && bob?.device(aliceMini)?.summary.currentUSD == 172.76)
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
    let frank = codex?.accounts.first?.cost
    check("frank（Codex）：服务器还不支持同步金额，有没有公开价格的模型", frank?.syncState == .oldServer && frank?.unpricedModels == ["codex-auto-review"]
          && frank?.summary.currentUSD == 544.31 && frank?.weeks.count == 13)
    if let t = s.costTotal {
        check("cost_total：统计过", t.scan.state == .ready && t.scan.updatedEpoch == 1794041880)
        check("cost_total：合计", t.summary.currentUSD == 1324.9 && t.summary.weeksUSD == 18160.52 && t.summary.averageUSD == 2048.52
              && t.summary.daysUSD == 8874.43 && t.summary.todayUSD == 96.83, "\(t.summary)")
        check("cost_total：按工具", t.byTool[.claude]?.currentUSD == 780.59 && t.byTool[.codex]?.daysUSD == 3320.53 && t.byTool.count == 2)
        check("cost_total：30 天，今天", t.days.count == 30 && t.today?.usd == 96.83 && t.today?.date == "2026-11-07")
        check("cost_total：认不出是哪个账号的", t.unattributed?.daysUSD == 12.3 && t.unattributed?.requests == 40)
        check("cost_total：价格表的日期", t.pricesDate == "2026-11-01" && t.currency == "USD")
    } else {
        check("state.json 有 cost_total", false)
    }
    if let n = loadFixture(StateDoc.self, "state_nocodex.json") {
        check("旧版 cca（没有 cost_total、features.cost、can.cost、cost）：都当作没有", n.costTotal == nil
              && n.tools.allSatisfy { !$0.features.cost && $0.accounts.allSatisfy { !$0.can.cost && $0.cost == nil } })
    }
    if let a = loadFixture(StateDoc.self, "state_agy.json") {
        check("Antigravity 没有折合金额", a.tool(.agy)?.features.cost == false && a.tool(.agy)?.accounts.allSatisfy { !$0.can.cost } == true)
    }
    if let x = loadFixture(StateDoc.self, "state_cost.json") {
        let alice = x.account(AccountRef(tool: .claude, key: "alice@example.com"))?.cost
        check("state_cost：只有这台设备，没开设备统计时 id 是 local、名字是主机名", alice?.devices.count == 1 && alice?.devices.first?.id == "local"
              && alice?.devices.first?.name == "test-mac" && alice?.syncState == .localOnly)
        check("state_cost：开始时还没有记录的两项是 partial、第一项是 0", alice?.weeks.prefix(2).allSatisfy(\.partial) == true
              && alice?.weeks.first?.usd == 0 && alice?.weeks.count == 13)
        check("state_cost：Codex 账号还没有记录，Codex 的平均是 null", x.account(AccountRef(tool: .codex, key: "dave@example.com"))?.cost == nil
              && x.costTotal?.byTool[.codex]?.averageUSD == nil && x.costTotal?.byTool[.codex]?.currentUSD == 0)
    }
    if let x = loadFixture(StateDoc.self, "state_cost_sync.json") {
        let alice = x.account(AccountRef(tool: .claude, key: "alice@example.com"))?.cost
        check("state_cost_sync：三台设备（这台、dan-air、撤销了的 old-air），还有一台没同步过金额",
              alice?.devices.map(\.name) == ["test-mac", "dan-air", "old-air"] && alice?.devices.last?.revoked == true
              && alice?.sync?.missing.map(\.name) == ["eve-mini"] && alice?.syncState == .ok)
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

    // 看谁的：整个账号或者一台设备（换了 state 以后不在了就回到整个账号）
    if let bob = loadFixture(StateDoc.self, "state.json")?.account(bobRef)?.cost {
        check("整个账号：柱子是 weeks 的 usd", CostChart.bars(bob, granularity: .week, scope: .account).map(\.usd) == bob.weeks.map(\.usd))
        check("一台设备：柱子是它的 weeks_usd、days_usd", CostChart.bars(bob, granularity: .week, scope: .device(aliceMini)).map(\.usd) == bob.device(aliceMini)?.weeksUSD
              && CostChart.bars(bob, granularity: .day, scope: .device(aliceMini)).map(\.usd) == bob.device(aliceMini)?.daysUSD)
        check("柱子跟着周期（日期、partial 是账号的）", CostChart.bars(bob, granularity: .day, scope: .device(aliceMini)).map(\.period) == bob.days)
        check("统计：整个账号的、那台设备的", CostChart.summary(bob, scope: .account) == bob.summary
              && CostChart.summary(bob, scope: .device(aliceMini)) == bob.device(aliceMini)?.summary)
        check("那台设备不在了：看整个账号", CostChart.resolve(.device("SHA256:gone"), in: bob) == .account
              && CostChart.bars(bob, granularity: .week, scope: .device("SHA256:gone")).map(\.usd) == bob.weeks.map(\.usd)
              && CostChart.resolve(.device(aliceMini), in: bob) == .device(aliceMini) && CostChart.resolve(.device(aliceMini), in: nil) == .account)
        let onlyMini = AccountCost(devices: bob.devices.filter { $0.id == aliceMini })
        check("只剩一台设备（没有选哪台的菜单）：看整个账号，不会卡在那台设备上", onlyMini.devices.count == 1
              && CostChart.resolve(.device(aliceMini), in: onlyMini) == .account)
        check("一台以上有记录的设备才列出来", CostChart.listsDevices(bob) && !CostChart.listsDevices(nil)
              && !CostChart.listsDevices(AccountCost(devices: Array(bob.devices.prefix(1)))))

        // 账号页「每周用量」的图右边留出和「折合金额」的大图的纵轴一样宽的一栏（同样的刻度撑开）：两张图的绘图区一样宽
        let gutter = CostChart.gutterLabels(bob, granularity: .week, scope: .account)
        check("每周用量的图右边那一栏：和折合金额的大图右边的刻度一样", gutter == ["$0", "$750", "$1,500"]
              && gutter == CostChart.axisLabels(top: CostChart.niceTop(CostChart.maxUSD([CostChart.bars(bob, granularity: .week, scope: .account)])),
                                                currency: bob.currency), "\(gutter)")
        let mini = bob.device(aliceMini)?.weeksUSD.max() ?? 0
        check("跟着折合金额看的那一方（一台设备有它自己的纵轴）、按日还是按周",
              CostChart.gutterLabels(bob, granularity: .week, scope: .device(aliceMini)) == CostChart.axisLabels(top: CostChart.niceTop(mini), currency: "USD")
              && CostChart.gutterLabels(bob, granularity: .day, scope: .account)
                == CostChart.axisLabels(top: CostChart.niceTop(bob.days.map(\.usd).max() ?? 0), currency: "USD"))
        let zero = AccountCost(weeks: bob.weeks.map { period($0.startEpoch, $0.endEpoch) })
        check("折合金额没画大图（还没有记录、都是 0）时不留", CostChart.gutterLabels(nil, granularity: .week, scope: .account).isEmpty
              && !CostChart.charted(CostChart.bars(zero, granularity: .week, scope: .account))
              && CostChart.gutterLabels(zero, granularity: .week, scope: .account).isEmpty)
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
    let bars = CostChart.bars(bob, granularity: .week, scope: .account)
    let detail = { (i: Int, scope: CostScope, g: CostGranularity) -> String in
        let list = CostChart.bars(bob, granularity: g, scope: scope)
        return Texts.costBarDetail(list[i], granularity: g, scope: scope, currency: "USD", now: now, calendar: sh)
    }
    check("指着本周", detail(12, .account, .week) == "本周到现在（11 月 3 日–10 日）：$518 · 2,327 次请求", detail(12, .account, .week))
    check("指着开始时还没有记录的一周：补一句", detail(4, .account, .week) == "9 月 8 日–15 日：$607 · 2,768 次请求；这一项开始时还没有记录，可能偏小",
          detail(4, .account, .week))
    check("有没有公开价格的模型的一周：补一句", detail(7, .account, .week) == "9 月 29 日–10 月 6 日：$785 · 3,609 次请求；其中 3 次请求的模型没有公开价格，没算钱",
          detail(7, .account, .week))
    check("还没有记录的一周：$0，不写请求数", detail(0, .account, .week) == "8 月 17 日–23 日：$0；这一项开始时还没有记录，可能偏小", detail(0, .account, .week))
    check("一台设备：没有请求数", detail(12, .device(aliceMini), .week) == "本周到现在（11 月 3 日–10 日）：$173", detail(12, .device(aliceMini), .week))
    check("指着今天、别的一天", detail(29, .account, .day) == "今天到现在：$44.06 · 191 次请求"
          && detail(0, .account, .day) == "10 月 9 日 周五：$185 · 864 次请求", detail(29, .account, .day) + " | " + detail(0, .account, .day))
    check("柱子的金额是 cca 给的", bars.map(\.usd) == bob.weeks.map(\.usd))
    check("图下面那一行（按周）", Texts.costCaption(bob.summary, granularity: .week, currency: "USD") == "本周 $518 · 平均每周 $976（7 周） · 近 12 周合计 $7,957",
          Texts.costCaption(bob.summary, granularity: .week, currency: "USD"))
    check("图下面那一行（按天）", Texts.costCaption(bob.summary, granularity: .day, currency: "USD") == "今天 $44.06 · 近 30 天合计 $4,284",
          Texts.costCaption(bob.summary, granularity: .day, currency: "USD"))
    check("没有平均（一周都没过完）时不写平均", Texts.costCaption(CostSummary(weeksUSD: 3, currentUSD: 3), granularity: .week, currency: "USD")
          == "本周 $3.00 · 近 12 周合计 $3.00")
    check("一台设备的平均不写几周（cca 没给）", Texts.costCaption(bob.device(aliceMini)!.summary, granularity: .week, currency: "USD")
          == "本周 $173 · 平均每周 $266 · 近 12 周合计 $2,173", Texts.costCaption(bob.device(aliceMini)!.summary, granularity: .week, currency: "USD"))
    check("价格的小字：按工具说是谁的价格", Texts.costFootnote([.claude], pricesDate: "2026-11-01")
          == "按 Anthropic 官方 API 的价格折算（价格表 2026 年 11 月 1 日），不是实际花费：订阅不按这个收费。"
          && Texts.costFootnote([.codex], pricesDate: nil) == "按 OpenAI 官方 API 的价格折算，不是实际花费：订阅不按这个收费。"
          && Texts.costFootnote([.claude, .codex, .claude], pricesDate: "2026-11-01").hasPrefix("按 Anthropic、OpenAI 官方 API 的价格折算（价格表 2026 年 11 月 1 日）")
          && Texts.costFootnote([], pricesDate: nil) == "按官方 API 的价格折算，不是实际花费：订阅不按这个收费。",
          Texts.costFootnote([], pricesDate: nil))

    let notes = { (c: AccountCost) in Texts.costNotes(c, now: now, calendar: sh) }
    check("同步过：还有没同步过金额的设备、很久没同步上来的设备（撤销了的不说）", notes(bob) == [
        CostNote(text: "还有 1 台设备没有算进来（carol-air，可能 cca 太旧）", warning: true),
        CostNote(text: "office-imac 周四 15:00 以后的还没同步上来", warning: true),
    ], "\(notes(bob))")
    let deviceNotes = { (c: AccountCost, id: String) in Texts.costNotes(c, scope: .device(id), now: now, calendar: sh) }
    let officeImac = "SHA256:" + String(repeating: "c", count: 43)
    check("看一台设备：只说它自己很久没同步上来；别的设备没算进来、没同步上来不说",
          deviceNotes(bob, officeImac) == [CostNote(text: "office-imac 周四 15:00 以后的还没同步上来", warning: true)]
          && deviceNotes(bob, aliceMini).isEmpty && deviceNotes(bob, me).isEmpty, "\(deviceNotes(bob, officeImac))")
    check("看一台设备：账号里没有这台设备时照整个账号说", deviceNotes(bob, "SHA256:gone") == notes(bob))
    let syncFailed = decodeCost(#"{"weeks":[],"days":[],"devices":[{"id":"m","name":"m","me":true},{"id":"o","name":"o"}],"#
                                + #""sync":{"state":"error","message":"x","missing":[{"name":"z"}]}}"#)
    check("看一台设备：别的设备同步来的才说同步的状态，这台设备自己的数字不经同步",
          syncFailed.map { deviceNotes($0, "o") } == [CostNote(text: "上次同步没成功：x", warning: true)]
          && syncFailed.map { deviceNotes($0, "m") }?.isEmpty == true && syncFailed.map { notes($0).count } == 2,
          "\(String(describing: syncFailed.map { deviceNotes($0, "o") }))")
    check("没登录这个账号", notes(carol) == [CostNote(text: "这台设备没登录这个账号，显示的是上次同步来的")], "\(notes(carol))")
    check("上次同步没成功（橙色，带原因）", notes(grace) == [CostNote(text: "上次同步没成功：连不上设备统计的服务器（https://192.0.2.10:18443）", warning: true)])
    check("服务器还不支持", notes(frank) == [CostNote(text: "只算了这台设备：维护者的设备统计服务器还不支持同步金额")])
    check("只有这台设备：不说", notes(erin).isEmpty)
    let pending = decodeCost(#"{"weeks":[],"days":[],"sync":{"state":"pending","missing":[]}}"#)
    let weird = decodeCost(#"{"weeks":[],"days":[],"sync":{"state":"later","missing":[{"name":"a"},{"name":"b"},{"name":"c"},{"name":"d"},{"name":""}]}}"#)
    let off = decodeCost(#"{"weeks":[],"days":[],"sync":{"state":"off","missing":[]}}"#)
    check("还没和别的设备同步过", pending.map { notes($0) } == [CostNote(text: "还没和别的设备同步过")])
    check("不认识的同步状态当没同步成；没名字的设备不算；超过三台写「等」", weird?.syncState == .error && weird?.sync?.state.rawValue == "later"
          && weird.map({ notes($0) }) == [CostNote(text: "上次同步没成功", warning: true),
                                  CostNote(text: "还有 4 台设备没有算进来（a、b、c 等，可能 cca 太旧）", warning: true)],
          "\(String(describing: weird.map { notes($0) }))")
    check("关掉了设备统计、cca 没给 sync：不说", off.map({ notes($0) })?.isEmpty == true && AccountCost().syncState == .localOnly && notes(AccountCost()).isEmpty)
    check("还没有记录", Texts.costEmpty(.claude) == "还没有记录：用 Claude Code 干过活以后会显示在这里。"
          && Texts.costEmpty(.codex) == "还没有记录：用 Codex 干过活以后会显示在这里。")
    check("从什么时候开始有记录", Texts.costSince(bob.sinceEpoch!, tool: .claude, now: now, calendar: sh)
          == "从 9 月 10 日开始有记录。Claude Code 2.1.317 起的会话才带着账号信息，更早的、Claude 桌面 App 里的会话算不进来。"
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
    check("认不出是哪个账号的", Texts.costUnattributed(s.costTotal?.unattributed, currency: "USD")
          == "近 30 天另有 $12.30 认不出是哪个账号的（旧版 Claude Code、桌面 App 里的会话），没算进来。"
          && Texts.costUnattributed(nil, currency: "USD") == nil)
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
    check("summary 宽容", messy?.summary.weeksUSD == 3.5 && messy?.summary.averageUSD == nil && messy?.summary.averageWeeks == 2)
    check("since_epoch 坏了当没有", messy?.sinceEpoch == nil)
    check("没有公开价格的模型：只要非空字符串", messy?.unpricedModels == ["m"])
    check("同步状态不是字符串：当没同步成；没名字的设备不算", messy?.syncState == .error && messy?.sync?.missing.map(\.name) == ["a"])
    check("没有数组的周期：空的", decodeCost(#"{}"#).map { $0.weeks.isEmpty && $0.days.isEmpty && $0.devices.isEmpty && $0.currency == "USD" } == true)
    let total = try? JSONDecoder().decode(CostTotal.self, from: Data(#"""
    {"current_usd":"2","average_usd":null,"days":[{"start_epoch":86400,"end_epoch":172800,"usd":1,"current":true},{"start_epoch":0,"end_epoch":86400,"usd":2}],
     "by_tool":{"claude":{"current_usd":2},"gemini":{"current_usd":5},"codex":"x"},"scan":{"state":"warming"}}
    """#.utf8))
    check("cost_total：顶层的几项、天按时间排好", total?.summary.currentUSD == 2 && total?.summary.averageUSD == nil
          && total?.days.map(\.startEpoch) == [0, 86400] && total?.today?.startEpoch == 86400 && total?.currency == "USD")
    check("cost_total：by_tool 只认认识的工具、坏掉的不要", total?.byTool.keys.map(\.rawValue).sorted() == ["claude"] && total?.byTool[.claude]?.currentUSD == 2)
    check("cost_total：不认识的统计状态原样留着（App 当统计过）", total?.scan.state.rawValue == "warming"
          && total?.scan.state.isKnown == false && total?.unattributed == nil)
    let noScan = try? JSONDecoder().decode(CostTotal.self, from: Data(#"{}"#.utf8))
    check("cost_total：没有 scan 当统计过", noScan?.scan.state == .ready && noScan?.byTool.isEmpty == true)
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
        ("/Users/me/.cca/cost/local.json", .state), ("/Users/me/.cca/cost/remote.json", .state), ("/Users/me/.cca/cost/sync.json", .state),
        ("/Users/me/.cca/cost/scan.json", nil), ("/Users/me/.cca/cost/local.json.tmp", nil), ("/Users/me/.cca/cost/.local.json.x1", nil),
        ("/Users/me/.cca/cost/cost.lock", nil), ("/Users/me/.cca/cost", nil), ("/Users/me/.cca/cost/old/local.json", nil),
        ("/Users/me/.cca/cost/usage.json", nil), ("/Users/me/.cca/accounts/a/cost/local.json", nil),
    ]
    let wrong = costCases.filter { CcaWatcher.classify(path: $0.0, homes: homes) != $0.1 }.map(\.0)
    check("看着 cost/ 里的 local.json、remote.json、sync.json（别的不算）", wrong.isEmpty, wrong.joined(separator: " "))

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
        m.selection = .usageStats
        m.statsMode = .cost
        m.refreshUsageIfNeeded()
        m.refreshCost()
        try? await Task.sleep(nanoseconds: 100_000_000)
        check("旧版 cca（hello 没有 cost）：不显示折合金额、不统计", !m.costAvailable && !m.costEnabled && !m.costVisible && syncs(t).isEmpty)
        m.shutdown()
    }
    // hello 的版本不认识：一样当作没有
    do {
        let (t, m) = model(hello: helloText.replacingOccurrences(of: #""cost": 1"#, with: #""cost": 2"#))
        await m.start()
        m.refreshCost()
        try? await Task.sleep(nanoseconds: 50_000_000)
        check("hello.cost 不是认识的版本：当作没有", !m.costAvailable && syncs(t).isEmpty)
        m.shutdown()
    }
    // cost_total 是 null（关掉了、没有可统计的账号）：不统计
    do {
        let (t, m) = model(state: costState { $0["cost_total"] = NSNull() })
        await m.start()
        m.setMainVisible(true)
        m.selection = .usageStats
        m.statsMode = .cost
        m.refreshUsageIfNeeded()
        m.refreshCost()
        try? await Task.sleep(nanoseconds: 100_000_000)
        check("cost_total 为 null：不显示、不统计", m.costAvailable && !m.costEnabled && !m.costVisible && syncs(t).isEmpty)
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
    check("用量统计页的额度模式：看不见金额，不统计", m.statsMode == .quota && !m.costVisible && syncs(t).isEmpty)
    m.statsMode = .cost
    let visibleNow = m.costVisible
    check("切到金额模式：看得见（5 分钟一次），马上统计一次（低优先级）", await waitUntil { syncs(t).count == 1 } && visibleNow
          && syncs(t).last?.background == true && syncs(t).last?.arguments == ["cost", "sync", "--json"])
    check("统计完重读 state", await waitUntil { !m.costSyncing })
    check("设置记下了", prefs.string(forKey: "stats.mode") == "cost")
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
    m.selection = .usageStats
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

    // 看谁的：整个账号或者一台设备（只记在内存里）；用量统计页点一行：打开账号页、滚到「折合金额」
    check("默认看整个账号", m.costScope(bobRef) == .account)
    m.setCostScope(bobRef, .device(aliceMini))
    check("换成一台设备", m.costScope(bobRef) == .device(aliceMini))
    m.setCostScope(bobRef, .device("SHA256:gone"))
    check("那台设备不在了：看整个账号", m.costScope(bobRef) == .account)
    m.selection = .usageStats
    m.showCost(bobRef, device: aliceMini)
    check("用量统计页点一台设备：打开账号页，看这台设备，滚到折合金额", m.selection == .account(bobRef) && m.costScope(bobRef) == .device(aliceMini)
          && m.costFocus == bobRef)
    m.costFocus = nil
    m.showCost(bobRef)
    check("点账号那一行：看整个账号", m.costScope(bobRef) == .account && m.costFocus == bobRef)
    m.costFocus = nil
    let frankRef = AccountRef(tool: .codex, key: "frank@example.com")
    m.showCost(frankRef)
    check("ChatGPT 账号：换到 Codex", m.tool == .codex && m.selection == .account(frankRef))
    m.tool = .claude
    let groups = m.costGroups()
    check("金额模式的分组：Claude、ChatGPT 的官方账号（第三方 API 账号不列），和总览一样的顺序",
          groups.map(\.department) == [.claude, .chatgpt]
          && groups.first?.accounts.map(\.key) == ["工作(bob@example.com)", "carol@example.com", "测试(dave@example.com)", "erin@example.com", "grace@example.com"]
          && groups.last?.accounts.map(\.key) == ["frank@example.com"], "\(groups.map { $0.accounts.map(\.key) })")
    m.costGranularity = .day
    check("按周 / 按天记在设置里", prefs.string(forKey: "cost.period") == "day")
    m.shutdown()
    let (_, again) = model(prefs: prefs)
    check("下次打开还是金额模式、按天", again.statsMode == .cost && again.costGranularity == .day)

    // 用户走开了、过热：不统计
    do {
        let idle = HookLog()
        idle.conditions.idleSeconds = UsagePolling.awayAfter + 1
        let (t2, m2) = model(log: idle)
        await m2.start()
        m2.setMainVisible(true)
        m2.selection = .usageStats
        m2.statsMode = .cost
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
        m3.selection = .usageStats
        m3.statsMode = .cost
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
