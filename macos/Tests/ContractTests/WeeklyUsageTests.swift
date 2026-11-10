import Foundation

// 每周用量（usage_weeks，PROTOCOL 2.2）：账号页的「每周用量」和侧栏的「用量统计」。数字都是 cca 算的，App 只解码、排版。

private let week = 7 * 86400.0

private func weeks(_ json: String) -> UsageWeeks? {
    try? JSONDecoder().decode(UsageWeeks.self, from: Data(json.utf8))
}

@MainActor
func weeklyUsageTests() async {
    section("每周用量")

    // 解码：坏掉的一周跳过（缺时间、缺百分比、区间是空的），数字写成字符串的也收；按时间排好；没有 summary 时是空的统计
    let raw = """
    {"since_epoch": 1790000000, "weeks": [
      {"start_epoch": 1793692800, "end_epoch": 1794297600, "used_percent": 41, "resets": 0, "last_epoch": 1794041100, "current": true, "partial": false},
      {"start_epoch": 1793088000, "end_epoch": 1793692800, "used_percent": "72.5", "resets": 2, "last_epoch": 1793691300, "current": false, "partial": true},
      {"start_epoch": 1792483200, "used_percent": 50},
      {"start_epoch": 1792483200, "end_epoch": 1793088000},
      {"start_epoch": 1793088000, "end_epoch": 1793088000, "used_percent": 10},
      {"start_epoch": 1791878400, "end_epoch": 1792483200, "used_percent": -3, "resets": -1, "future_field": [1, 2]},
      null, "x"
    ]}
    """
    let uw = weeks(raw)
    check("usage_weeks：解码", uw != nil)
    if let uw {
        check("坏掉的一周跳过，其余按时间排好", uw.weeks.map(\.endEpoch) == [1792483200, 1793692800, 1794297600], "\(uw.weeks.map(\.endEpoch))")
        check("数字写成字符串的也收；重置次数、没记全照给的", uw.weeks[1].usedPercent == 72.5 && uw.weeks[1].resets == 2 && uw.weeks[1].partial)
        check("负数当 0；缺的布尔是 false", uw.weeks[0].usedPercent == 0 && uw.weeks[0].resets == 0 && !uw.weeks[0].current
              && !uw.weeks[0].partial && uw.weeks[0].lastEpoch == nil)
        check("还没过完的这一周", uw.current?.endEpoch == 1794297600 && uw.current?.usedPercent == 41)
        check("没有 summary：空的统计", uw.summary == UsageWeeksSummary() && uw.summary.averagePercent == nil)
        check("since_epoch", uw.sinceEpoch == 1790000000)
    }
    check("没有还没过完的一周", weeks(#"{"weeks": [{"start_epoch": 1, "end_epoch": 2, "used_percent": 3}]}"#)?.current == nil)
    check("weeks 不是数组：空的", weeks(#"{"since_epoch": 1, "weeks": "x", "summary": {"weeks": 2, "average_percent": 40}}"#)
          .map { $0.weeks.isEmpty && $0.summary.weeks == 2 } == true)
    check("summary：类型不对的字段当 0 / 没有", weeks(#"{"weeks": [], "summary": {"weeks": "3", "average_percent": "x", "maxed": 1.5, "under_half": -2}}"#)?.summary
          == UsageWeeksSummary(weeks: 3, averagePercent: nil, maxed: 0, underHalf: 0))
    check("账号上的 usage_weeks：null、缺、类型不对都是没有",
          Account.sample(["key": "a@example.com", "usage_weeks": NSNull()]).usageWeeks == nil
          && Account.sample(["key": "a@example.com"]).usageWeeks == nil
          && Account.sample(["key": "a@example.com", "usage_weeks": "x"]).usageWeeks == nil)
    check("账号上的 usage_weeks：解码",
          Account.sample(["key": "a@example.com", "usage_weeks": ["since_epoch": 5, "weeks": [], "summary": ["weeks": 0]]]).usageWeeks
          == UsageWeeks(sinceEpoch: 5, weeks: [], summary: UsageWeeksSummary()))

    // 演示数据：bob 记了 12 周（一周没记全），frank（Codex）有一周中间重置过，超过 100%；carol 最近没查，没有本周；没查过、API 账号没有
    if let d = fixture("state.json"), let s = try? DocumentDecoder.decode(StateDoc.self, from: d) {
        func acct(_ t: ToolID, _ k: String) -> Account? { s.tool(t)?.accounts.first { $0.key == k } }
        let bob = acct(.claude, "工作(bob@example.com)")?.usageWeeks
        check("演示数据：bob 12 周，统计是过完的 11 周", bob?.weeks.count == 12
              && bob?.summary == UsageWeeksSummary(weeks: 11, averagePercent: 84, maxed: 3, underHalf: 0)
              && bob?.current?.usedPercent == 41 && bob?.weeks.filter(\.partial).map(\.usedPercent) == [64],
              "\(String(describing: bob?.summary))")
        let frank = acct(.codex, "frank@example.com")?.usageWeeks
        check("演示数据：frank 有一周重置过一次，用到 130%", frank?.weeks.contains { $0.resets == 1 && $0.usedPercent == 130 } == true
              && frank?.summary == UsageWeeksSummary(weeks: 5, averagePercent: 93, maxed: 2, underHalf: 0))
        let carol = acct(.claude, "carol@example.com")?.usageWeeks
        check("演示数据：carol 只有以前的几周，没有本周", carol?.weeks.count == 3 && carol?.current == nil && carol?.summary.weeks == 3)
        check("演示数据：没查过的 dave、第三方 API 账号没有", acct(.claude, "测试(dave@example.com)")?.usageWeeks == nil
              && (s.tool(.claude)?.accounts ?? []).filter(\.isAPI).allSatisfy { $0.usageWeeks == nil })
    } else {
        check("读演示数据", false)
    }

    // 文字（东八区；演示数据的现在是 2026-11-07 周六 17:00）
    var sh = Calendar(identifier: .gregorian)
    sh.timeZone = TimeZone(identifier: "Asia/Shanghai")!
    let now = 1794042000.0
    check("统计：平均、几周用完、几周没用到一半",
          Texts.weeksSummary(UsageWeeksSummary(weeks: 8, averagePercent: 63, maxed: 2, underHalf: 3)) == "记下的 8 周平均用到 63% · 2 周用完 · 3 周没用到一半")
    check("统计：只有 1 周不说平均；都没有时只有平均",
          Texts.weeksSummary(UsageWeeksSummary(weeks: 1, averagePercent: 87)) == "记下的 1 周用到 87%"
          && Texts.weeksSummary(UsageWeeksSummary(weeks: 4, averagePercent: 70)) == "记下的 4 周平均用到 70%")
    check("统计：一周都还没过完时没有", Texts.weeksSummary(UsageWeeksSummary()) == nil && Texts.weeksAverage(UsageWeeksSummary()) == nil
          && Texts.weeksSummary(UsageWeeksSummary(weeks: 2, averagePercent: nil)) == nil)
    check("用量统计页的平均和小字", Texts.weeksAverage(UsageWeeksSummary(weeks: 3, averagePercent: 52)) == "52%"
          && Texts.weeksNotes(UsageWeeksSummary(weeks: 3, averagePercent: 52, maxed: 0, underHalf: 1)) == ["1 周没用到一半"]
          && Texts.weeksNotes(UsageWeeksSummary(weeks: 3, averagePercent: 90, maxed: 2)) == ["2 周用完"]
          && Texts.weeksNotes(UsageWeeksSummary(weeks: 3, averagePercent: 70)).isEmpty)
    let current = UsageWeek(startEpoch: 1793692800, endEpoch: 1794297600, usedPercent: 41, current: true)
    check("本周：到现在用了多少、什么时候重置", Texts.weekDetail(current, now: now, calendar: sh) == "本周到现在用了 41%，周二 16:00 重置",
          Texts.weekDetail(current, now: now, calendar: sh))
    check("本周中间重置过", Texts.weekDetail(UsageWeek(startEpoch: now - 86400, endEpoch: now + 8010, usedPercent: 130.4, resets: 1, current: true),
                                       now: now, calendar: sh) == "本周到现在用了 130%（中间重置过 1 次），2 小时 13 分后重置")
    let past = UsageWeek(startEpoch: 1792483200, endEpoch: 1793088000, usedPercent: 87.5)
    check("过完的一周：哪几天、用到多少", Texts.weekDetail(past, now: now, calendar: sh) == "10 月 20 日–27 日：用到 88%",
          Texts.weekDetail(past, now: now, calendar: sh))
    check("跨月", Texts.weekDetail(UsageWeek(startEpoch: 1793088000, endEpoch: 1793692800, usedPercent: 72, resets: 2), now: now, calendar: sh)
          == "10 月 27 日–11 月 3 日：用到 72%，中间重置过 2 次")
    check("没记全：实际可能更多", Texts.weekDetail(UsageWeek(startEpoch: 1790668800, endEpoch: 1791273600, usedPercent: 64, partial: true),
                                         now: now, calendar: sh) == "9 月 29 日–10 月 6 日：用到 64%；结束前一天没查到，实际可能更多")
    check("不是今年的带年份", Texts.weekRange(UsageWeek(startEpoch: 1766908800, endEpoch: 1766908800 + week, usedPercent: 1), now: now, calendar: sh)
          == "2025 年 12 月 28 日–1 月 4 日")
    check("柱子下面的日期：这一周从哪天开始，还没过完的写本周", Texts.weekLabel(past, calendar: sh) == "10/20" && Texts.weekLabel(current, calendar: sh) == "本周")
    check("从什么时候开始记", Texts.weeksSince(1787047200, now: now, calendar: sh) == "从 8 月 18 日开始记")

    // 用量统计页：两个工具的官方账号，和总览一样按用户排的顺序；第三方 API 账号不在里面；切工具、重读 state 都还在这一页
    guard let helloData = fixture("hello.json"), let stateData = fixture("state.json") else {
        check("读演示数据", false)
        return
    }
    let hello = String(decoding: helloData, as: UTF8.self), state = String(decoding: stateData, as: UTF8.self)
    func model(_ hello: String) async -> AppModel {
        let t = ScriptedTransport { request, _ in
            if request.arguments == ["_gui", "hello"] { return response(0, stdout: hello) }
            if request.arguments == ["_gui", "state"] { return response(0, stdout: state) }
            return response(0, stdout: SampleDocs.clientUpdates)
        }
        let m = makeModel(t, prefs: MemoryPrefs())
        await m.start()
        return m
    }
    let m = await model(hello)
    let groups = m.officialGroups()
    check("用量统计：Claude、ChatGPT 两组", groups.map(\.department) == [.claude, .chatgpt], "\(groups.map(\.department))")
    check("用量统计：只有官方账号，顺序和总览、侧栏一样", groups.allSatisfy { $0.accounts.allSatisfy { !$0.isAPI } }
          && groups.first?.accounts.map(\.key) == m.accountGroups(.claude).first?.accounts.map(\.key)
          && groups.last?.accounts.map(\.key) == m.accounts(.codex).filter { !$0.isAPI }.map(\.key))
    m.selection = .usageStats
    await m.loadState()
    check("重读 state：还在用量统计页", m.selection == .usageStats)
    m.tool = .codex
    check("切工具：还在用量统计页", m.selection == .usageStats && m.tool == .codex)
    m.shutdown()

    let noCodex = hello.replacingOccurrences(of: #""codex": {"enabled": true"#, with: #""codex": {"enabled": false"#)
    check("改出没启用 Codex 的 hello", noCodex != hello)
    let m2 = await model(noCodex)
    check("没启用 Codex：只有 Claude 一组", m2.officialGroups().map(\.department) == [.claude])
    m2.shutdown()
}
