import Foundation

// MARK: - 通用不变量（两种模式都跑）

@discardableResult
func genericHello(_ name: String, _ d: Data) -> HelloDoc? {
    guard let doc = decodeOrFail(HelloDoc.self, name, d) else { return nil }
    let raw = jsonObject(d) as? [String: Any] ?? [:]
    check("\(name) version 1", doc.version == 1, "\(String(describing: doc.version))")
    check("\(name) kind hello", doc.kind == "hello", doc.kind ?? "nil")
    check("\(name) api 在 App 支持的范围", GUIAPI.isSupported(doc.api), "api=\(String(describing: doc.api))")
    check("\(name) cca_version 不为空", !(doc.ccaVersion ?? "").isEmpty)
    check("\(name) home 不为空", !(doc.home ?? "").isEmpty)
    check("\(name) installed 是布尔", isBool(raw["installed"]))
    check("\(name) dev_mode 是布尔", isBool(raw["dev_mode"]))
    check("\(name) tools.claude.enabled 恒为 true", doc.tools[.claude]?.enabled == true)
    let codex = (raw["tools"] as? [String: Any])?["codex"] as? [String: Any]
    check("\(name) tools.codex.enabled 是布尔", isBool(codex?["enabled"]))
    if let agy = (raw["tools"] as? [String: Any])?["agy"] as? [String: Any] {   // 旧版 cca 没有 tools.agy
        check("\(name) tools.agy.enabled 是布尔", isBool(agy["enabled"]))
    }
    check("\(name) 启用的工具里一定有 Claude", doc.enabledTools.first == .claude)
    if let v = raw["cost"] {   // 有折合金额（PROTOCOL 2.1）；旧版 cca 没有这个字段
        check("\(name) cost 是 1", (v as? NSNumber)?.intValue == 1 && doc.cost == 1, "\(v)")
    }
    if let v = raw["install_cli"] {   // 有辅助安装（cca install-cli）；旧版 cca 没有这个字段
        check("\(name) install_cli 是 1", (v as? NSNumber)?.intValue == 1 && doc.installCLI == 1, "\(v)")
    }
    let gui = raw["gui"] as? [String: Any]
    check("\(name) gui.app 是 ~/Applications/cca.app", (gui?["app"] as? String)?.hasSuffix("/Applications/cca.app") == true,
          "\(String(describing: gui?["app"]))")
    knownEnums(name, auditHello(doc))
    return doc
}

/// usage_weeks（PROTOCOL 2.2）：数字都是 cca 算的，App 只排版；这里核对 cca 给的东西自洽
func usageWeeksInvariants(_ n: String, _ raw: [String: Any], _ uw: UsageWeeks?) {
    guard let uw else {
        check("\(n) 解码", false)
        return
    }
    let rawWeeks = raw["weeks"] as? [[String: Any]]
    check("\(n) weeks 是对象数组，每一周都解码了", rawWeeks?.count == uw.weeks.count,
          "\(rawWeeks?.count ?? -1) vs \(uw.weeks.count)")
    check("\(n) since_epoch 是数字", isNumber(raw["since_epoch"]) && uw.sinceEpoch != nil)
    let ends = (rawWeeks ?? []).compactMap { ($0["end_epoch"] as? NSNumber)?.doubleValue }
    check("\(n) 按时间排好", ends.count == uw.weeks.count && ends == ends.sorted())
    check("\(n) 字段的类型", (rawWeeks ?? []).allSatisfy {
        isNumber($0["used_percent"]) && isNumber($0["resets"]) && isBool($0["current"]) && isBool($0["partial"]) && isNumber($0["last_epoch"])
    })
    check("\(n) 一周不超过 7 天", uw.weeks.allSatisfy { $0.endEpoch - $0.startEpoch <= 7 * 86400 })
    let current = uw.weeks.filter(\.current)
    check("\(n) 还没过完的最多一周，是最后一项", current.count <= 1 && (current.isEmpty || uw.weeks.last?.current == true))
    check("\(n) 还没过完的不算没记全", uw.weeks.allSatisfy { !($0.current && $0.partial) })
    let s = uw.summary
    check("\(n) summary 统计的是 weeks 里过完的那几周", s.weeks == uw.weeks.count - current.count, "\(s.weeks)")
    check("\(n) 用完的、没用到一半的加起来不超过总数", s.maxed + s.underHalf <= s.weeks)
    check("\(n) 有过完的周才有平均", (s.weeks == 0) == (s.averagePercent == nil) && isNumber((raw["summary"] as? [String: Any])?["weeks"]))
}

func usageEntryInvariants(_ n: String, _ ru: [String: Any], _ u: UsageEntry?) {
    guard let u else {
        check("\(n) 解码", false)
        return
    }
    check("\(n) key 不为空", !(u.key ?? "").isEmpty)
    check("\(n) error 和 error_code 同时有或同时没有", (u.error == nil) == (u.errorCode == nil),
          "error=\(String(describing: u.error)) code=\(String(describing: u.errorCode))")
    check("\(n) stale 是布尔", isBool(ru["stale"]))
    if let p = ru["outdated_login_plan"], !isNull(p) {
        check("\(n) outdated_login_plan 不是 null 时是不为空的文字", (p as? String).map { !$0.isEmpty } == true, "\(p)")
    }
    if let f = ru["use_first"], !isNull(f) {
        // 周结算提醒：区间到本周的重置时间为止，还剩至少 20%（规则在 cca：24 小时、20%）
        check("\(n) use_first 解码", u.useFirst != nil, "\(f)")
        if let uf = u.useFirst {
            check("\(n) use_first 区间到本周的重置时间为止", uf.fromEpoch < uf.untilEpoch && uf.untilEpoch == u.sevenDay?.resetsEpoch,
                  "\(uf) \(String(describing: u.sevenDay?.resetsEpoch))")
            check("\(n) use_first 还剩至少 20%", (20...100).contains(uf.leftPercent), "\(uf.leftPercent)")
        }
    }
    if u.source == .none {
        check("\(n) source none 没有窗口", u.fiveHour == nil && u.sevenDay == nil)
    }
    if ru["fetched_at"] is String {
        check("\(n) 有 fetched_at 就有数字 fetched_epoch", isNumber(ru["fetched_epoch"]))
    }
    var windows: [(String, [String: Any])] = []
    for (k, v) in ru where k == "five_hour" || k.hasPrefix("seven_day") {
        if let w = v as? [String: Any] { windows.append((k, w)) }
    }
    for (i, m) in ((ru["model_scoped"] as? [Any]) ?? []).enumerated() {
        if let w = m as? [String: Any] { windows.append(("model_scoped[\(i)]", w)) }
    }
    for (k, w) in windows {
        if !isNull(w["resets_at"]) {
            check("\(n) \(k) 有 resets_at 时 resets_epoch 是数字（cca 认不出的时间写法是 null）", isNumber(w["resets_epoch"]) || isNull(w["resets_epoch"]))
        }
        check("\(n) \(k) reset 是布尔", isBool(w["reset"]))
        if let p = (w["utilization"] as? NSNumber)?.doubleValue {
            check("\(n) \(k) utilization 合理", p.isFinite && p >= 0, "\(p)")
        }
    }
}

/// 一个数是不是不小于 0 的数字
private func nonNegative(_ x: Any?) -> Bool {
    guard isNumber(x), let v = (x as? NSNumber)?.doubleValue else { return false }
    return v.isFinite && v >= 0
}

/// 不小于 0 的整数
private func wholeCount(_ x: Any?) -> Bool {
    guard nonNegative(x), let v = (x as? NSNumber)?.doubleValue else { return false }
    return v == v.rounded()
}

/// 金额加起来的误差：cca 每一项各自四舍五入，加起来以后再四舍五入（按分算也够宽）
private func near(_ a: Double, _ b: Double, terms: Int) -> Bool {
    abs(a - b) <= 0.01 * Double(max(terms, 1)) + 0.001
}

/// 周期、天（PROTOCOL 2.2 的 cost.weeks / days、cost_total.days）：首尾相接
private func contiguous(_ list: [CostPeriod]) -> Bool {
    zip(list, list.dropFirst()).allSatisfy { $0.endEpoch == $1.startEpoch }
}

/// 最近 30 个本地日历日：30 项、首尾相接、一天 23 到 25 小时（夏令时）、最后一项是今天（只有它是 current）、日期按顺序
private func thirtyDays(_ n: String, _ days: [CostPeriod], _ raw: [[String: Any]]?, now: Double?) {
    check("\(n) days 是对象数组，每一天都解码了", raw?.count == days.count, "\(raw?.count ?? -1) vs \(days.count)")
    check("\(n) days 是最近 30 天", days.count == 30, "\(days.count)")
    check("\(n) days 首尾相接", contiguous(days))
    check("\(n) 一天 23 到 25 小时", days.allSatisfy { (82800...90000).contains($0.endEpoch - $0.startEpoch) })
    check("\(n) 最后一天是今天，只有它是 current", days.last?.current == true && days.filter(\.current).count == 1)
    if let now, let today = days.last {
        check("\(n) 今天包含 generated_at", today.startEpoch <= now && now < today.endEpoch, "\(today.startEpoch)…\(today.endEpoch) vs \(now)")
    }
    let dates = days.compactMap(\.date)
    check("\(n) 每一天都有 YYYY-MM-DD 的 date，按顺序、不重复", dates.count == days.count
          && dates.allSatisfy { Texts.costDateParts($0) != nil } && zip(dates, dates.dropFirst()).allSatisfy { $0 < $1 })
    check("\(n) days 的 current、usd 的类型", (raw ?? []).allSatisfy { isBool($0["current"]) && nonNegative($0["usd"]) })
}

/// 折合金额（PROTOCOL 2.2 的账号 `cost`）：数字都是 cca 算的，App 只排版；这里核对 cca 给的东西自洽（只查 PROTOCOL 写明的）
func costInvariants(_ n: String, _ raw: [String: Any], _ c: AccountCost?, now: Double?) {
    guard let c else {
        check("\(n) 解码", false)
        return
    }
    check("\(n) currency 是 USD", raw["currency"] as? String == "USD", "\(String(describing: raw["currency"]))")
    check("\(n) prices_date 是 YYYY-MM-DD", (raw["prices_date"] as? String).flatMap { Texts.costDateParts($0) } != nil,
          "\(String(describing: raw["prices_date"]))")
    check("\(n) since_epoch 是数字或 null", isNumber(raw["since_epoch"]) || isNull(raw["since_epoch"]))
    let rawWeeks = raw["weeks"] as? [[String: Any]], rawDays = raw["days"] as? [[String: Any]]
    let w = c.weeks, d = c.days
    // 周期
    check("\(n) weeks 是对象数组，每一项都解码了", rawWeeks?.count == w.count, "\(rawWeeks?.count ?? -1) vs \(w.count)")
    let ends = (rawWeeks ?? []).compactMap { ($0["end_epoch"] as? NSNumber)?.doubleValue }
    check("\(n) weeks 按时间排好、首尾相接", ends.count == w.count && ends == ends.sorted() && contiguous(w))
    check("\(n) weeks 最多 13 项", w.count <= 13, "\(w.count)")
    if let now {
        check("\(n) weeks 只列结束在最近 12 周里的", w.allSatisfy { $0.endEpoch >= now - 12 * 7 * 86400 - 3601 })
    }
    check("\(n) 不到一天的零头并进了相邻的那一项", w.count < 2 || w.allSatisfy { $0.endEpoch - $0.startEpoch >= 86400 })
    let current = w.filter(\.current)
    check("\(n) 还没过完的最多一项，是最后一项", current.count <= 1 && (current.isEmpty || w.last?.current == true))
    if let now {
        check("\(n) 还没过完的还没结束，别的都结束了", w.allSatisfy { $0.current ? $0.endEpoch > now : $0.endEpoch <= now })
    }
    check("\(n) weeks 的 basis 是字符串", (rawWeeks ?? []).allSatisfy { $0["basis"] is String })
    thirtyDays(n, d, rawDays, now: now)
    let rawPeriods = (rawWeeks ?? []) + (rawDays ?? [])
    check("\(n) 金额、请求数不小于 0", rawPeriods.allSatisfy {
        nonNegative($0["usd"]) && wholeCount($0["requests"]) && wholeCount($0["unpriced_requests"]) && isBool($0["current"]) && isBool($0["partial"])
    })
    check("\(n) 没有公开价格的请求不多于请求数", (w + d).allSatisfy { $0.unpricedRequests <= $0.requests })
    check("\(n) tokens 是四个不小于 0 的整数", rawPeriods.allSatisfy { p in
        guard let t = p["tokens"] else { return true }
        guard let o = t as? [String: Any] else { return false }
        return ["input", "output", "cache_read", "cache_write"].allSatisfy { wholeCount(o[$0]) }
    })
    // partial：since_epoch 落在这一项里面或者后面（正好是开始的那一刻两种都可以）
    if let since = c.sinceEpoch {
        check("\(n) partial 和 since_epoch 一致", (w + d).allSatisfy { p in
            since > p.startEpoch ? p.partial : since < p.startEpoch ? !p.partial : true
        }, "since=\(since)")
    }
    // 统计
    let s = c.summary, rs = raw["summary"] as? [String: Any] ?? [:]
    check("\(n) summary 的字段类型", ["weeks_usd", "current_usd", "days_usd", "today_usd"].allSatisfy { nonNegative(rs[$0]) }
          && ["average_weeks", "average_days", "requests", "unpriced_requests"].allSatisfy { wholeCount(rs[$0]) }
          && (nonNegative(rs["average_usd"]) || isNull(rs["average_usd"])) && (nonNegative(rs["average_day_usd"]) || isNull(rs["average_day_usd"])))
    check("\(n) weeks_usd 是列出的周期加起来", near(s.weeksUSD, w.map(\.usd).reduce(0, +), terms: w.count), "\(s.weeksUSD)")
    check("\(n) current_usd 是当前那一项", near(s.currentUSD, current.first?.usd ?? 0, terms: 1), "\(s.currentUSD)")
    check("\(n) days_usd 是 30 天加起来", near(s.daysUSD, d.map(\.usd).reduce(0, +), terms: d.count), "\(s.daysUSD)")
    check("\(n) today_usd 是今天", near(s.todayUSD, d.last(where: \.current)?.usd ?? 0, terms: 1), "\(s.todayUSD)")
    check("\(n) requests、unpriced_requests 是列出的周期加起来", s.requests == w.map(\.requests).reduce(0, +)
          && s.unpricedRequests == w.map(\.unpricedRequests).reduce(0, +))
    // 平均：过完的、不是 partial、至少 6 天的周期（夏令时差一小时的两种都可以）
    func eligible(_ minLength: Double) -> [Int] {
        w.indices.filter { !w[$0].current && !w[$0].partial && w[$0].endEpoch - w[$0].startEpoch >= minLength }
    }
    let strict = eligible(6 * 86400 + 3601), loose = eligible(6 * 86400 - 3601)
    check("\(n) average_weeks 是过完的、不是 partial、至少 6 天的几项", strict.count <= s.averageWeeks && s.averageWeeks <= loose.count,
          "\(s.averageWeeks) vs \(strict.count)…\(loose.count)")
    check("\(n) 一项都没有时 average_usd 为 null", (s.averageUSD == nil) == (s.averageWeeks == 0))
    if strict == loose, !strict.isEmpty, let avg = s.averageUSD {
        check("\(n) average_usd 是这几项的平均", near(avg, strict.map { w[$0].usd }.reduce(0, +) / Double(strict.count), terms: strict.count),
              "\(avg)")
    }
    // 按天的平均：过完的、不是 partial 的那几天（今天不算）
    let goodDays = d.indices.filter { !d[$0].current && !d[$0].partial }
    check("\(n) average_days 是过完的、不是 partial 的几天", s.averageDays == goodDays.count, "\(s.averageDays) vs \(goodDays.count)")
    check("\(n) 一天都没有时 average_day_usd 为 null", (s.averageDayUSD == nil) == goodDays.isEmpty)
    if let avg = s.averageDayUSD, !goodDays.isEmpty {
        check("\(n) average_day_usd 是这几天的平均", near(avg, goodDays.map { d[$0].usd }.reduce(0, +) / Double(goodDays.count), terms: goodDays.count),
              "\(avg)")
    }
    // 设备
    let rawDevices = raw["devices"] as? [[String: Any]]
    check("\(n) devices 是对象数组，每一台都解码了", rawDevices?.count == c.devices.count, "\(rawDevices?.count ?? -1) vs \(c.devices.count)")
    let ids = c.devices.map(\.id)
    check("\(n) 设备的 id、名字不为空，id 不重复", ids.allSatisfy { !$0.isEmpty } && Set(ids).count == ids.count
          && (rawDevices ?? []).allSatisfy { !(($0["name"] as? String) ?? "").isEmpty })
    check("\(n) 这台设备最多一台，排在最前", c.devices.dropFirst().allSatisfy { !$0.isMe })
    check("\(n) id 是 local 的只能是这台设备", c.devices.allSatisfy { $0.id != "local" || $0.isMe })
    let others = c.devices.filter { !$0.isMe }
    check("\(n) 别的设备按 30 天的金额从多到少", zip(others, others.dropFirst()).allSatisfy { $0.summary.daysUSD + 0.005 >= $1.summary.daysUSD })
    for (j, rd) in (rawDevices ?? []).enumerated() {
        let dn = "\(n) 设备[\(j)]"
        check("\(dn) me、revoked、stale 是布尔，synced_epoch 是数字或 null", isBool(rd["me"]) && isBool(rd["revoked"]) && isBool(rd["stale"])
              && (isNumber(rd["synced_epoch"]) || isNull(rd["synced_epoch"])))
        let rw = rd["weeks_usd"] as? [Any], rdy = rd["days_usd"] as? [Any]
        check("\(dn) weeks_usd、days_usd 和 weeks、days 一一对应", rw?.count == rawWeeks?.count && rdy?.count == rawDays?.count,
              "\(rw?.count ?? -1)/\(rawWeeks?.count ?? -1) \(rdy?.count ?? -1)/\(rawDays?.count ?? -1)")
        check("\(dn) 金额不小于 0", ((rw ?? []) + (rdy ?? [])).allSatisfy { nonNegative($0) })
        let ds = rd["summary"] as? [String: Any] ?? [:]
        check("\(dn) summary 的字段类型", ["weeks_usd", "current_usd", "days_usd", "today_usd"].allSatisfy { nonNegative(ds[$0]) }
              && ["average_weeks", "average_days"].allSatisfy { wholeCount(ds[$0]) }
              && (nonNegative(ds["average_usd"]) || isNull(ds["average_usd"])) && (nonNegative(ds["average_day_usd"]) || isNull(ds["average_day_usd"])))
    }
    check("\(n) 每一项是各台设备加起来", w.indices.allSatisfy { i in
        near(c.devices.map { $0.weeksUSD[i] }.reduce(0, +), w[i].usd, terms: c.devices.count + 1)
    } && d.indices.allSatisfy { i in near(c.devices.map { $0.daysUSD[i] }.reduce(0, +), d[i].usd, terms: c.devices.count + 1) })
    let currentIndex = w.lastIndex(where: \.current), todayIndex = d.lastIndex(where: \.current)
    for dv in c.devices {
        let ds = dv.summary, dn = "\(n) \(dv.name)"
        check("\(dn) 的 summary 和它的数组一致", near(ds.weeksUSD, dv.weeksUSD.reduce(0, +), terms: w.count)
              && near(ds.daysUSD, dv.daysUSD.reduce(0, +), terms: d.count)
              && near(ds.currentUSD, currentIndex.map { dv.weeksUSD[$0] } ?? 0, terms: 1)
              && near(ds.todayUSD, todayIndex.map { dv.daysUSD[$0] } ?? 0, terms: 1), "\(ds)")
        check("\(dn) 有没有平均和整个账号一样（算的是同样几项、几天）", (ds.averageUSD == nil) == (s.averageUSD == nil)
              && (ds.averageDayUSD == nil) == (s.averageDayUSD == nil) && ds.averageWeeks == s.averageWeeks && ds.averageDays == s.averageDays)
        if strict == loose, !strict.isEmpty, let avg = ds.averageUSD {
            check("\(dn) 的平均是同样几项的平均", near(avg, strict.map { dv.weeksUSD[$0] }.reduce(0, +) / Double(strict.count), terms: strict.count),
                  "\(avg)")
        }
        if let avg = ds.averageDayUSD, !goodDays.isEmpty {
            check("\(dn) 的平均每天是同样几天的平均", near(avg, goodDays.map { dv.daysUSD[$0] }.reduce(0, +) / Double(goodDays.count), terms: goodDays.count),
                  "\(avg)")
        }
    }
    let devAvg = c.devices.compactMap(\.summary.averageUSD), devDay = c.devices.compactMap(\.summary.averageDayUSD)
    check("\(n) 各台设备的平均加起来是整个账号的", (s.averageUSD.map { near($0, devAvg.reduce(0, +), terms: devAvg.count + 1) } ?? devAvg.isEmpty)
          && (s.averageDayUSD.map { near($0, devDay.reduce(0, +), terms: devDay.count + 1) } ?? devDay.isEmpty))
    // 没有公开价格的模型、同步
    check("\(n) unpriced_models 是字符串数组", (raw["unpriced_models"] as? [Any])?.allSatisfy { $0 is String } == true)
    check("\(n) 有没有公开价格的请求就列出模型", s.unpricedRequests == 0 || !c.unpricedModels.isEmpty)
    guard let sync = raw["sync"] as? [String: Any] else {
        check("\(n) 有 sync", false)
        return
    }
    check("\(n) sync 的字段类型", sync["state"] is String && (sync["message"] is String || isNull(sync["message"]))
          && (isNumber(sync["checked_epoch"]) || isNull(sync["checked_epoch"])))
    let missing = sync["missing"] as? [[String: Any]]
    check("\(n) sync.missing 是对象数组，都有名字", missing != nil && (missing ?? []).allSatisfy {
        !(($0["name"] as? String) ?? "").isEmpty && (isNumber($0["used_epoch"]) || isNull($0["used_epoch"]))
    } && missing?.count == c.sync?.missing.count)
    if c.sync?.state == .ok { check("\(n) 同步过（ok）就有 checked_epoch", c.sync?.checkedEpoch != nil) }
    if c.sync?.state == .error { check("\(n) 没同步成（error）就有原因", c.sync?.message != nil) }
}

/// `cost_total`（PROTOCOL 2.2）：所有账号加起来（按 start_epoch 对上每一天），by_tool 只列启用了的工具，加起来是总数
func costTotalInvariants(_ n: String, _ raw: [String: Any], _ doc: StateDoc, now: Double?) {
    guard let t = doc.costTotal else {
        check("\(n) 解码", false)
        return
    }
    check("\(n) currency 是 USD", raw["currency"] as? String == "USD", "\(String(describing: raw["currency"]))")
    check("\(n) prices_date 是 YYYY-MM-DD", (raw["prices_date"] as? String).flatMap { Texts.costDateParts($0) } != nil)
    let accounts = doc.tools.flatMap(\.accounts)
    let enabled = Set(doc.tools.filter(\.enabled).map(\.id.rawValue))
    /// 一份（顶层：所有设备；me：这台设备）：rows 是各账号的那一份（工具、统计、30 天），加起来应该是 part
    func part(_ label: String, _ raw: [String: Any], _ p: CostTotalPart, _ rows: [(tool: ToolID, s: CostSummary, days: [CostPeriod])]) {
        check("\(label) 金额的类型", ["current_usd", "weeks_usd", "days_usd", "today_usd"].allSatisfy { nonNegative(raw[$0]) }
              && (nonNegative(raw["average_usd"]) || isNull(raw["average_usd"])) && (nonNegative(raw["average_day_usd"]) || isNull(raw["average_day_usd"])))
        thirtyDays(label, p.days, raw["days"] as? [[String: Any]], now: now)
        func sums(_ list: [CostSummary], _ s: CostSummary, _ label: String) {
            check("\(label) current、weeks、days、today 是各账号加起来",
                  near(s.currentUSD, list.map(\.currentUSD).reduce(0, +), terms: list.count)
                  && near(s.weeksUSD, list.map(\.weeksUSD).reduce(0, +), terms: list.count)
                  && near(s.daysUSD, list.map(\.daysUSD).reduce(0, +), terms: list.count)
                  && near(s.todayUSD, list.map(\.todayUSD).reduce(0, +), terms: list.count), "\(s)")
            let avgs = list.compactMap(\.averageUSD), days = list.compactMap(\.averageDayUSD)
            check("\(label) average_usd、average_day_usd 是有平均值的账号的平均加起来，一个都没有为 null",
                  (s.averageUSD == nil) == avgs.isEmpty && (s.averageUSD.map { near($0, avgs.reduce(0, +), terms: avgs.count) } ?? true)
                  && (s.averageDayUSD == nil) == days.isEmpty && (s.averageDayUSD.map { near($0, days.reduce(0, +), terms: days.count) } ?? true),
                  "\(String(describing: s.averageUSD)) \(String(describing: s.averageDayUSD))")
        }
        sums(rows.map(\.s), p.summary, label)
        check("\(label) 每一天是各账号那一天加起来", p.days.allSatisfy { day in
            let parts = rows.compactMap { r in r.days.first { $0.startEpoch == day.startEpoch }?.usd }
            return near(day.usd, parts.reduce(0, +), terms: parts.count)
        })
        let rawByTool = raw["by_tool"] as? [String: Any]
        check("\(label) by_tool 是对象，只列启用了的工具", rawByTool != nil && Set(rawByTool?.keys.map { $0 } ?? []).isSubset(of: enabled),
              (rawByTool?.keys.sorted() ?? []).joined(separator: ","))
        for (tool, s) in p.byTool {
            sums(rows.filter { $0.tool == tool }.map(\.s), s, "\(label) by_tool.\(tool.rawValue)")
        }
        if !p.byTool.isEmpty {
            let parts = Array(p.byTool.values)
            check("\(label) by_tool 加起来是总数", near(p.summary.currentUSD, parts.map(\.currentUSD).reduce(0, +), terms: parts.count)
                  && near(p.summary.weeksUSD, parts.map(\.weeksUSD).reduce(0, +), terms: parts.count)
                  && near(p.summary.daysUSD, parts.map(\.daysUSD).reduce(0, +), terms: parts.count)
                  && near(p.summary.todayUSD, parts.map(\.todayUSD).reduce(0, +), terms: parts.count))
        }
    }
    part(n, raw, t.all, accounts.compactMap { a in a.cost.map { (a.tool, $0.summary, $0.days) } })
    // me：各账号里这台设备的那一份加起来（这台设备没有记录的账号不算）；按工具分开的工具和顶层一样
    guard let rawMe = raw["me"] as? [String: Any], let me = t.me else {
        check("\(n) 有 me", false)
        return
    }
    part(n + " me", rawMe, me, accounts.compactMap { a in
        a.cost.flatMap { c in c.me.map { d in (a.tool, d.summary, c.days.enumerated().map { i, p in
            CostPeriod(startEpoch: p.startEpoch, endEpoch: p.endEpoch, date: p.date, current: p.current, usd: i < d.daysUSD.count ? d.daysUSD[i] : 0)
        }) } }
    })
    check("\(n) me 和顶层的工具一样、天一样", Set(me.byTool.keys) == Set(t.all.byTool.keys) && me.days.map(\.startEpoch) == t.all.days.map(\.startEpoch))
    check("\(n) 这台设备的不比所有设备的多", me.summary.weeksUSD <= t.all.summary.weeksUSD + 0.01 && me.summary.daysUSD <= t.all.summary.daysUSD + 0.01
          && zip(me.days, t.all.days).allSatisfy { $0.usd <= $1.usd + 0.01 })
    let scan = raw["scan"] as? [String: Any]
    check("\(n) scan 的字段类型", scan?["state"] is String && (isNumber(scan?["updated_epoch"]) || isNull(scan?["updated_epoch"])))
    if t.scan.state == .ready { check("\(n) 统计过（ready）就有 updated_epoch", t.scan.updatedEpoch != nil) }
    if let u = raw["unattributed"], !isNull(u) {
        let o = u as? [String: Any]
        check("\(n) unattributed 是不小于 0 的金额和请求数", nonNegative(o?["days_usd"]) && wholeCount(o?["requests"]))
    }
}

/// `cca cost sync --json`（PROTOCOL 3.5）：统计和同步得怎么样（金额本身在 `_gui state` 里）
@discardableResult
func genericCostSync(_ name: String, _ d: Data) -> CostSyncDoc? {
    guard let doc = decodeOrFail(CostSyncDoc.self, name, d) else { return nil }
    let raw = jsonObject(d) as? [String: Any] ?? [:]
    check("\(name) version 1", doc.version == 1)
    check("\(name) kind cost_sync", doc.kind == "cost_sync")
    check("\(name) generated_epoch 是数字", isNumber(raw["generated_epoch"]))
    let scan = raw["scan"] as? [String: Any], sync = raw["sync"] as? [String: Any]
    check("\(name) 有 scan、sync", scan != nil && sync != nil)
    if let scan {
        check("\(name) scan 的字段类型", scan["state"] is String && (isNumber(scan["updated_epoch"]) || isNull(scan["updated_epoch"]))
              && wholeCount(scan["files"]) && wholeCount(scan["read_bytes"]) && wholeCount(scan["records"]) && isBool(scan["first"]))
        if doc.scan?.state == .ready { check("\(name) 统计好了（ready）就有 updated_epoch", doc.scan?.updatedEpoch != nil) }
    }
    if let sync {
        check("\(name) sync 的字段类型", sync["state"] is String && (sync["message"] is String || isNull(sync["message"]))
              && (isNumber(sync["checked_epoch"]) || isNull(sync["checked_epoch"])))
        if doc.sync?.state == .error { check("\(name) 没同步成（error）就有原因", doc.sync?.message != nil) }
        if doc.sync?.state == .ok { check("\(name) 同步成了（ok）就有 checked_epoch", doc.sync?.checkedEpoch != nil) }
    }
    let a = EnumAudit()
    a.add("scan.state", doc.scan?.state)
    a.add("sync.state", doc.sync?.state)
    knownEnums(name, a.unknown)
    return doc
}

func accountInvariants(_ name: String, _ t: ToolState, _ a: Account, _ ra: [String: Any], now: Double? = nil) {
    let n = "\(name) \(t.id.rawValue)/\(a.key)"
    check("\(n) tool 填对了", a.tool == t.id)
    check("\(n) check 总是可用", a.can.check)
    for k in a.canWhy.keys {
        let off = (k == "app" && !a.can.app) || (k == "web" && !a.can.web) || (k == "usage" && !a.can.usage)
        check("\(n) can_why.\(k) 对应一个不可用的按钮", off)
    }
    if let can = ra["can"] as? [String: Any], can["billing"] != nil {
        check("\(n) 账单页的条件和网页版一样（Antigravity 没有账单页）", a.can.billing == (a.can.web && t.id != .agy))
    }
    if let can = ra["can"] as? [String: Any], can["limit_resets"] != nil {
        check("\(n) 用量页只有 Claude 账号有，其余条件和网页版一样", a.can.limitResets == (t.id == .claude && a.can.web))
    }
    if let raw = ra["api_key"], !isNull(raw) {
        // API key 只给掩码：前缀（最多 8 个字符）+ … + 最后 4 个字符，短的 key 没有掩码；没存的掩码、来源都是空的
        let k = a.apiKey
        check("\(n) api_key 解码", k != nil, "\(raw)")
        if let k {
            check("\(n) api_key 的掩码不是 key", k.hint.isEmpty || k.hint.range(of: "^[!-~]{0,8}…[!-~]{4}$", options: .regularExpression) != nil, k.hint)
            check("\(n) 没存的 api_key 没有掩码和来源", k.saved || (k.hint.isEmpty && k.source == .none))
            check("\(n) api_key 的来源认得", k.source.isKnown, k.source.rawValue)
            check("\(n) 账号表里写的 key 只有第三方 API 账号有", k.source != .table || a.kind == .api)
            if let rk = raw as? [String: Any], rk["shared"] != nil {
                // 同步（PROTOCOL 2.2）：映射里的官方账号、开着设备统计（和「设备」一样），Antigravity 不同步；来源、时间只给记下的
                check("\(n) 会同步的和有「设备」的是同一批账号（Antigravity 除外）", k.shared == (a.can.devices && t.id != .agy))
                check("\(n) 同步来源、记下的时间只给存着的 key", k.saved || (k.fromDevice == nil && k.savedEpoch == nil))
                check("\(n) 同步的原因只给会同步的账号", k.shared || k.syncError == nil)
                check("\(n) 第三方 API 账号不同步", a.kind != .api || (!k.shared && k.fromDevice == nil))
            }
        }
    }
    if a.kind == .api {
        check("\(n) API 账号 login 为 api", a.login == .api, a.login.rawValue)
        check("\(n) API 账号 badge 为 null", a.badge == nil && isNull(ra["badge"]))
        check("\(n) API 账号 usage 为 null", a.usage == nil && isNull(ra["usage"]))
        check("\(n) API 账号 email、plan 为空", (a.email ?? "").isEmpty && (a.plan ?? "").isEmpty)
        let api = ra["api"] as? [String: Any]
        check("\(n) API 账号有 api 对象", api != nil)
        let bad = (api ?? [:]).keys.filter {
            $0.range(of: "key|secret|token|password|env", options: [.regularExpression, .caseInsensitive]) != nil
        }
        check("\(n) api 对象绝不带 key", bad.isEmpty, bad.joined(separator: ","))
        if a.port == nil {
            check("\(n) 直连的 API 账号 node/slot/group 为 null", a.node == nil && a.slot == nil && a.group == nil)
        }
    } else {
        check("\(n) 官方账号 api 为 null", a.api == nil && isNull(ra["api"]))
        check("\(n) 官方账号有 badge", a.badge != nil && !(a.badge?.label.isEmpty ?? true))
        check("\(n) badge 颜色是 #RRGGBB", a.badge?.rgb != nil, a.badge?.color ?? "nil")
        check("\(n) 官方账号的 email 是字符串（键只写别名时为空）", a.email != nil)
    }
    if !isNull(ra["usage"]) {
        check("\(n) usage 只给登录着的官方账号", a.login == .loggedIn && a.kind != .api, a.login.rawValue)
        check("\(n) state 里 usage 的 source 是 cache 或 none", a.usage?.source == .cache || a.usage?.source == UsageSource.none,
              a.usage?.source.rawValue ?? "nil")
        usageEntryInvariants(n + " usage", ra["usage"] as? [String: Any] ?? [:], a.usage)
    }
    if a.kind == .api {
        check("\(n) API 账号没有 usage_weeks", a.usageWeeks == nil && isNull(ra["usage_weeks"]))
    }
    if let raw = ra["usage_weeks"], !isNull(raw) {
        usageWeeksInvariants(n + " usage_weeks", raw as? [String: Any] ?? [:], a.usageWeeks)
    }
    // 折合金额（PROTOCOL 2.2）：只给 can.cost 为真的账号（Claude、Codex 的官方账号，工具的 features.cost），没有记录时为 null
    if let can = ra["can"] as? [String: Any], can["cost"] != nil {
        check("\(n) can.cost 是布尔", isBool(can["cost"]))
        if a.can.cost { check("\(n) 有折合金额的是官方账号、工具有折合金额", !a.isAPI && t.features.cost) }
    }
    if a.kind == .api {
        check("\(n) API 账号没有折合金额", a.cost == nil && isNull(ra["cost"]) && !a.can.cost)
    }
    if let raw = ra["cost"], !isNull(raw) {
        check("\(n) 有 cost 的账号 can.cost 为真", a.can.cost)
        costInvariants(n + " cost", raw as? [String: Any] ?? [:], a.cost, now: now)
    }
    if a.login == .emailMismatch {
        check("\(n) email_mismatch 带 login_email", !(a.loginEmail ?? "").isEmpty)
    }
    if a.pinned {
        check("\(n) pinned 带 pinned_node", !(a.pinnedNode ?? "").isEmpty)
    }
    if a.port != nil {
        check("\(n) 有端口就有 slot、group、node", a.slot != nil && a.group != nil && a.node != nil)
    }
    check("\(n) app.running 是布尔", isBool((ra["app"] as? [String: Any])?["running"]))
    if let tier = ra["plan_tier"] {   // 旧版 cca 没有（当作不知道）
        check("\(n) plan_tier 是字符串", tier is String)
        if a.kind == .api || a.login != .loggedIn || (a.plan ?? "").isEmpty {
            check("\(n) 没登录、登错号、API 账号、没套餐时 plan_tier 为空", a.planTier == .unknown, a.planTier.rawValue)
        }
    }
    if let cli = ra["cli"] as? [String: Any] {   // 旧版 cca 没有（当作没有会话）
        let running = cli["running"] as? Bool, sessions = (cli["sessions"] as? NSNumber)?.intValue ?? -1
        check("\(n) cli.running 是布尔", isBool(cli["running"]))
        check("\(n) cli.sessions 是不小于 0 的整数", isNumber(cli["sessions"]) && sessions >= 0)
        check("\(n) cli.running 等于 sessions > 0", running == (sessions > 0))
        check("\(n) cli.since_epoch 只在有会话时是数字", running == true ? isNumber(cli["since_epoch"]) : isNull(cli["since_epoch"]))
        check("\(n) cli 解码", a.cli?.running == running && a.cliSessions == max(sessions, 0))
    }
    if let h = ra["health"] as? [String: Any] {
        for k in ["version", "checked_at", "age_seconds", "stale", "applies"] {
            check("\(n) health.\(k) 有值", !isNull(h[k]))
        }
        check("\(n) health.checked_at 是数字", isNumber(h["checked_at"]))
        check("\(n) health 解码", a.health != nil)
    }
}

func routingInvariants(_ name: String, _ r: Routing, _ doc: StateDoc, _ raw: [String: Any]) {
    let n = "\(name) routing"
    let pool = r.poolSize ?? -1
    check("\(n) pool_size 是 8 的倍数", pool > 0 && pool % 8 == 0, "\(pool)")
    check("\(n) 槽位数等于 pool_size", r.slots.count == pool, "\(r.slots.count) vs \(pool)")
    check("\(n) 槽位都解码了", ((raw["slots"] as? [Any])?.count ?? -1) == r.slots.count)
    if pool > 0 { check("\(n) 槽位号是 1…pool_size", r.slots.map(\.slot) == Array(1...pool)) }
    check("\(n) sync 是字符串", raw["sync"] is String)
    check("\(n) local_sync 是字符串", raw["local_sync"] is String)
    let first = r.firstPort ?? 7901
    for s in r.slots {
        let sn = "\(n) 槽位 \(s.slot)"
        check("\(sn) 端口 = 7900 + 槽位", s.port == first - 1 + s.slot && first == 7901,
              "port=\(String(describing: s.port)) first=\(first)")
        check("\(sn) 组名 CCA-\(s.slot)", s.group == "CCA-\(s.slot)", s.group ?? "nil")
        check("\(sn) tool/key 同时为空或同时有", (s.tool == nil) == (s.key == nil))
        if s.isEmpty {
            check("\(sn) 空槽位的 node 是 REJECT", s.node == "REJECT", s.node ?? "nil")
        } else if let ref = s.ref {
            let a = doc.account(ref)
            check("\(sn) 的账号存在", a != nil, ref.description)
            if let a {
                check("\(sn) 和账号的端口、槽位、组、节点一致",
                      a.port == s.port && a.slot == s.slot && a.group == s.group && a.node == s.node,
                      "account port=\(String(describing: a.port)) slot=\(String(describing: a.slot)) node=\(String(describing: a.node))")
            }
        }
    }
    for a in doc.allAccounts {
        if let n2 = a.slot {
            check("\(n) \(a.tool.rawValue)/\(a.key) 的槽位指回它", r.slot(n2)?.ref == a.ref)
        }
    }
    let names = r.nodes.map(\.name)
    check("\(n) 节点不重复", Set(names).count == names.count)
    for node in r.nodes {
        for u in node.users {
            let a = doc.account(u)
            check("\(n) 节点 \(node.name) 的用户存在", a != nil, u.description)
            if let a { check("\(n) 节点 \(node.name) 的用户确实用它", a.node == node.name, a.node ?? "nil") }
        }
    }
}

@discardableResult
func genericState(_ name: String, _ d: Data) -> StateDoc? {
    guard let doc = decodeOrFail(StateDoc.self, name, d) else { return nil }
    let raw = jsonObject(d) as? [String: Any] ?? [:]
    check("\(name) version 1", doc.version == 1)
    check("\(name) kind state", doc.kind == "state")
    check("\(name) generated_at 是数字", isNumber(raw["generated_at"]))
    check("\(name) cca_version 不为空", !(doc.ccaVersion ?? "").isEmpty)
    check("\(name) errors 是数组", raw["errors"] is [Any])
    check("\(name) notices 是数组", raw["notices"] is [Any])
    check("\(name) errors、notices 都解码了",
          (raw["errors"] as? [Any])?.count == doc.errors.count && (raw["notices"] as? [Any])?.count == doc.notices.count)
    check("\(name) 有 config", doc.config != nil)
    let ids = doc.tools.map(\.id)
    check("\(name) 工具不重复", Set(ids).count == ids.count)
    check("\(name) 有 claude 且启用", doc.tool(.claude)?.enabled == true)
    let rawTools = raw["tools"] as? [[String: Any]] ?? []
    check("\(name) tools 都解码了", rawTools.count == doc.tools.count)
    for (i, t) in doc.tools.enumerated() {
        let rt = i < rawTools.count ? rawTools[i] : [:]
        let rawAccounts = rt["accounts"] as? [[String: Any]] ?? []
        check("\(name) \(t.id.rawValue) 账号都解码了", rawAccounts.count == t.accounts.count,
              "\(rawAccounts.count) vs \(t.accounts.count)")
        let keys = t.accounts.map(\.key)
        check("\(name) \(t.id.rawValue) 账号键不重复", Set(keys).count == keys.count)
        check("\(name) \(t.id.rawValue) chrome 是布尔", isBool(rt["chrome"]))
        check("\(name) \(t.id.rawValue) settings_problems 是字符串数组",
              (rt["settings_problems"] as? [Any])?.allSatisfy { $0 is String } ?? false)
        check("\(name) \(t.id.rawValue) errors 是对象数组", (rt["errors"] as? [Any])?.allSatisfy { $0 is [String: Any] } ?? false)
        if t.id == .claude { check("\(name) claude 的 cli_prefix 是空的", t.cliPrefix.isEmpty) }
        if t.id == .codex { check("\(name) codex 的 cli_prefix 是 [codex]", t.cliPrefix == ["codex"]) }
        if let f = rt["features"] as? [String: Any] {   // 旧版 cca 没有 features
            check("\(name) \(t.id.rawValue) features 都是布尔", ["app", "api", "usage"].allSatisfy { isBool(f[$0]) }
                  && (f["cost"] == nil || isBool(f["cost"])))
        }
        if t.id == .agy {
            check("\(name) agy 的 cli_prefix 是 [agy]", t.cliPrefix == ["agy"])
            check("\(name) agy 没有桌面 App、第三方 API 账号、查用量的接口", t.features == ToolFeatures(app: false, api: false, usage: false))
            check("\(name) agy 的账号都是官方账号，查不了用量（unsupported）、开不了 App，没有账单页、用量页",
                  t.accounts.allSatisfy { !$0.isAPI && !$0.can.usage && $0.canWhy["usage"] == .unsupported && !$0.can.app
                      && $0.canWhy["app"] == .unsupported && !$0.can.billing && !$0.can.limitResets && $0.usage == nil })
        }
        if let last = t.last { check("\(name) \(t.id.rawValue) last 是表里的账号", t.accounts.contains { $0.key == last }, last) }
        for (j, a) in t.accounts.enumerated() {
            accountInvariants(name, t, a, j < rawAccounts.count ? rawAccounts[j] : [:], now: (raw["generated_at"] as? NSNumber)?.doubleValue)
        }
    }
    if let r = doc.routing {
        routingInvariants(name, r, doc, raw["routing"] as? [String: Any] ?? [:])
    } else {
        check("\(name) 有 routing", false)
    }
    // cost_total（PROTOCOL 2.2）：旧版 cca 没有；关掉了、没有能算的账号时为 null（不是 null 时一定有 can.cost 的账号）
    if let rt = raw["cost_total"], !isNull(rt) {
        check("\(name) cost_total 不是 null 时有能算折合金额的账号", doc.tools.contains { $0.accounts.contains { $0.can.cost } })
        costTotalInvariants(name + " cost_total", rt as? [String: Any] ?? [:], doc, now: (raw["generated_at"] as? NSNumber)?.doubleValue)
    }
    var secrets: [String] = []
    walkStrings(raw) { if looksSecret($0) { secrets.append($0) } }
    check("\(name) 没有像密钥的字符串", secrets.isEmpty, secrets.joined(separator: ", "))
    knownEnums(name, auditState(doc))
    return doc
}

@discardableResult
func genericLive(_ name: String, _ d: Data) -> LiveDoc? {
    guard let doc = decodeOrFail(LiveDoc.self, name, d) else { return nil }
    let raw = jsonObject(d) as? [String: Any] ?? [:]
    check("\(name) version 1", doc.version == 1)
    check("\(name) kind live", doc.kind == "live")
    check("\(name) generated_at 是数字", isNumber(raw["generated_at"]))
    check("\(name) route 是字符串", raw["route"] is String)
    check("\(name) 槽位都解码了", ((raw["slots"] as? [Any])?.count ?? -1) == doc.slots.count)
    let nums = doc.slots.map(\.slot)
    check("\(name) 槽位号不重复", Set(nums).count == nums.count)
    for s in doc.slots {
        let sn = "\(name) 槽位 \(s.slot)"
        check("\(sn) 端口 = 7900 + 槽位", s.port == 7900 + s.slot, "\(String(describing: s.port))")
        check("\(sn) tool/key 同时为空或同时有", (s.tool == nil) == (s.key == nil))
        check("\(sn) 空槽位才是 empty", (s.state == .empty) == (s.tool == nil), s.state.rawValue)
    }
    if let sum = doc.summary {
        let count = { (st: SlotState) in doc.slots.filter { $0.state == st }.count }
        check("\(name) summary 和槽位一致",
              sum.ok == count(.ok) && sum.drift == count(.drift) && sum.closed == count(.closed)
                && sum.unknown == count(.unknown) && sum.empty == count(.empty),
              "\(sum)")
    }
    let assigned = doc.slots.filter { $0.tool != nil }
    if doc.route == .router && doc.router?.reachable == false {
        check("\(name) 路由器连不上时已分配的槽位是 closed / unknown",
              assigned.allSatisfy { $0.state == .closed || $0.state == .unknown })
        check("\(name) 路由器连不上时 router.error 是 unreachable", doc.router?.error?.code == .unreachable)
    }
    if doc.route == Route.none {
        check("\(name) route none 时已分配的槽位是 unknown", assigned.allSatisfy { $0.state == .unknown })
    }
    if let e = doc.router?.error { check("\(name) router.error.code 不为空", !e.code.rawValue.isEmpty) }
    if let e = doc.local?.error { check("\(name) local.error.code 不为空", !e.code.rawValue.isEmpty) }
    // proxy_host（新字段，旧版没有）：看本机时是 127.0.0.1，route none 时为 null
    if raw.keys.contains("proxy_host") {
        check("\(name) proxy_host 是字符串或 null", isNull(raw["proxy_host"]) || raw["proxy_host"] is String)
        if doc.route == .local { check("\(name) 看本机时 proxy_host 是 127.0.0.1", doc.proxyHost == "127.0.0.1", doc.proxyHost ?? "nil") }
        if doc.route == Route.none { check("\(name) route none 时 proxy_host 为 null", doc.proxyHost == nil) }
        if doc.route == .router { check("\(name) 看路由器时有 proxy_host", !(doc.proxyHost ?? "").isEmpty) }
    }
    knownEnums(name, auditLive(doc))
    return doc
}

@discardableResult
func genericUsage(_ name: String, _ d: Data) -> UsageDoc? {
    guard let doc = decodeOrFail(UsageDoc.self, name, d) else { return nil }
    let raw = jsonObject(d) as? [String: Any] ?? [:]
    check("\(name) version 1", doc.version == 1)
    check("\(name) kind usage", doc.kind == "usage", doc.kind ?? "nil")
    let rawAccounts = raw["accounts"] as? [[String: Any]] ?? []
    check("\(name) 账号都解码了", rawAccounts.count == doc.accounts.count)
    let keys = doc.accounts.compactMap(\.key)
    check("\(name) 账号键不重复", Set(keys).count == keys.count && keys.count == doc.accounts.count)
    for (i, u) in doc.accounts.enumerated() {
        usageEntryInvariants("\(name) [\(u.key ?? String(i))]", i < rawAccounts.count ? rawAccounts[i] : [:], u)
    }
    check("\(name) skipped_api_accounts 是字符串数组", (raw["skipped_api_accounts"] as? [Any])?.allSatisfy { $0 is String } ?? false)
    if name.contains("codex") {
        check("\(name) Codex 的输出带 tool: codex", doc.tool == .codex)
    }
    check("\(name) 有 generated_epoch", isNumber(raw["generated_epoch"]))
    knownEnums(name, auditUsage(doc))
    return doc
}

@discardableResult
func genericCheck(_ name: String, _ d: Data) -> CheckDoc? {
    guard let doc = decodeOrFail(CheckDoc.self, name, d) else { return nil }
    let raw = jsonObject(d) as? [String: Any] ?? [:]
    check("\(name) version 1", doc.version == 1)
    check("\(name) kind check", doc.kind == "check")
    check("\(name) generated_at 是数字", isNumber(raw["generated_at"]))
    check("\(name) route 是字符串或 null", isNull(raw["route"]) || raw["route"] is String)
    let rawAccounts = raw["accounts"] as? [[String: Any]] ?? []
    check("\(name) 账号都解码了", rawAccounts.count == doc.accounts.count)
    for (i, a) in doc.accounts.enumerated() {
        let ra = i < rawAccounts.count ? rawAccounts[i] : [:]
        check("\(name) \(a.key) 有 health", a.health != nil)
        let h = ra["health"] as? [String: Any]
        check("\(name) \(a.key) health 带 version、checked_at", !isNull(h?["version"]) && isNumber(h?["checked_at"]))
    }
    check("\(name) shared_exit_ips 是字符串数组", (raw["shared_exit_ips"] as? [Any])?.allSatisfy { $0 is String } ?? false)
    knownEnums(name, auditCheck(doc))
    return doc
}

@discardableResult
func genericReset(_ name: String, _ d: Data) -> ResetDoc? {
    guard let doc = decodeOrFail(ResetDoc.self, name, d) else { return nil }
    let raw = jsonObject(d) as? [String: Any] ?? [:]
    check("\(name) version 1", doc.version == 1)
    check("\(name) kind reset", doc.kind == "reset")
    check("\(name) 有 generated_epoch", isNumber(raw["generated_epoch"]))
    check("\(name) key、message 不为空", !(doc.key ?? "").isEmpty && !(doc.message ?? "").isEmpty)
    check("\(name) outcome 是重置成了的几种（没有重置机会时 cca 不输出文档）",
          doc.outcome.map { [.reset, .alreadyRedeemed, .nothingToReset].contains($0) } ?? false, doc.outcome?.rawValue ?? "nil")
    if let ru = raw["usage"] as? [String: Any] {
        usageEntryInvariants("\(name) usage", ru, doc.usage)
        let rc = ru["reset_credits"] as? [String: Any] ?? [:]
        let credits = rc["credits"] as? [[String: Any]] ?? []
        check("\(name) 每一次重置机会都有 expires_epoch（数字或 null）", credits.allSatisfy { isNumber($0["expires_epoch"]) || isNull($0["expires_epoch"]) })
        check("\(name) cca 给了最早过期的 next_expires_epoch（数字或 null）", rc.keys.contains("next_expires_epoch")
              && (isNumber(rc["next_expires_epoch"]) || isNull(rc["next_expires_epoch"])))
    }
    let a = EnumAudit()
    a.add("tool", doc.tool)
    a.add("outcome", doc.outcome)
    a.usage("usage", doc.usage)
    knownEnums(name, a.unknown)
    return doc
}

/// `devices KEY --json`（PROTOCOL 3.4）：账号连着的设备，时间是 *_epoch，这台 Mac 排第一。
@discardableResult
func genericDevices(_ name: String, _ d: Data) -> DevicesDoc? {
    guard let doc = decodeOrFail(DevicesDoc.self, name, d) else { return nil }
    let raw = jsonObject(d) as? [String: Any] ?? [:]
    check("\(name) version 1", doc.version == 1)
    check("\(name) kind devices", doc.kind == "devices")
    check("\(name) generated_epoch 是数字", isNumber(raw["generated_epoch"]))
    let rawAccounts = raw["accounts"] as? [[String: Any]] ?? []
    check("\(name) 账号都解码了", rawAccounts.count == doc.accounts.count)
    for (i, a) in doc.accounts.enumerated() {
        let rd = (i < rawAccounts.count ? rawAccounts[i]["devices"] as? [[String: Any]] : nil) ?? []
        check("\(name) \(a.key) 的设备都解码了", rd.count == a.devices.count)
        check("\(name) \(a.key) 这台 Mac 排第一", a.devices.dropFirst().allSatisfy { !$0.isMe })
        for (j, dev) in a.devices.enumerated() {
            let r = j < rd.count ? rd[j] : [:]
            check("\(name) \(dev.name) 的时间是数字或 null", ["since_epoch", "last_active_epoch", "last_used_epoch", "seen_epoch"]
                .allSatisfy { isNull(r[$0]) || isNumber(r[$0]) })
            check("\(name) \(dev.name) 开着会话的才有开始时间", dev.state == .none || dev.state == .offline || dev.sinceEpoch != nil)
        }
    }
    knownEnums(name, auditDevices(doc))
    return doc
}

@discardableResult
func genericUpdate(_ name: String, _ d: Data) -> UpdateDoc? {
    guard let doc = decodeOrFail(UpdateDoc.self, name, d) else { return nil }
    let raw = jsonObject(d) as? [String: Any] ?? [:]
    check("\(name) version 1", doc.version == 1)
    check("\(name) kind update", doc.kind == "update")
    check("\(name) 有 generated_epoch", isNumber(raw["generated_epoch"]))
    check("\(name) cca_version、latest 是版本号", AppVersion(doc.ccaVersion ?? "") != nil && AppVersion(doc.latest ?? "") != nil,
          "\(doc.ccaVersion ?? "nil") \(doc.latest ?? "nil")")
    check("\(name) available 是布尔", isBool(raw["available"]))
    check("\(name) source 是 releases 或 latest（latest 时没有更新说明）", doc.source == .releases || (doc.source == .latest && doc.notes == nil))
    if let c = doc.ccaVersion.flatMap(AppVersion.init), let l = doc.latest.flatMap(AppVersion.init) {
        check("\(name) available 和版本号的先后一致（同号的测试版比正式版旧）", doc.available == (l > c))
    }
    check("\(name) 测试版的 channel 是 beta，正式版是 stable",
          doc.channel == ((AppVersion(doc.ccaVersion ?? "")?.isBeta ?? false) ? .beta : .stable))
    check("\(name) notes、url、published_epoch 是字符串 / 数字或 null",
          (isNull(raw["notes"]) || raw["notes"] is String) && (isNull(raw["url"]) || raw["url"] is String)
          && (isNull(raw["published_epoch"]) || isNumber(raw["published_epoch"])))
    let a = EnumAudit()
    a.add("channel", doc.channel)
    a.add("source", doc.source)
    knownEnums(name, a.unknown)
    return doc
}

@discardableResult
func genericEvents(_ name: String, _ d: Data) -> [CcaEvent] {
    let lines = String(decoding: d, as: UTF8.self).split(separator: "\n", omittingEmptySubsequences: true).map(String.init)
        .filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
    check("\(name) 不是空的", !lines.isEmpty)
    var evs: [CcaEvent] = []
    for (i, line) in lines.enumerated() {
        let ln = "\(name):\(i + 1)"
        let raw = jsonObject(Data(line.utf8)) as? [String: Any]
        check("\(ln) 是 JSON 对象", raw != nil)
        check("\(ln) v = 1", (raw?["v"] as? NSNumber)?.intValue == 1)
        check("\(ln) 有 ev", !((raw?["ev"] as? String) ?? "").isEmpty)
        guard let e = EventLog.parseLine(Data(line.utf8)) else {
            check("\(ln) 能解析", false)
            continue
        }
        evs.append(e)
        switch e {
        case .consent(let c):
            let item = raw?["item"] as? String ?? "", scope = raw?["scope"] as? String ?? ""
            check("\(ln) consent 的 grant 是 item:scope", c.grant == (scope.isEmpty ? item : "\(item):\(scope)"), c.grant)
            check("\(ln) consent 有 title、question", !c.title.isEmpty && !c.question.isEmpty)
            let def = raw?["default"] as? String
            check("\(ln) consent 的 default 是 yes / no", def == "yes" || def == "no", def ?? "nil")
            check("\(ln) consent 的 severity 是字符串", raw?["severity"] is String)
        case .terminal(let t):
            check("\(ln) terminal 的 argv 不为空", !t.argv.isEmpty && (raw?["argv"] as? [Any])?.allSatisfy { $0 is String } == true)
            check("\(ln) terminal 有 display", !((raw?["display"] as? String) ?? "").isEmpty)
            check("\(ln) terminal 有 code", !((raw?["code"] as? String) ?? "").isEmpty)
        case .fail(let f):
            check("\(ln) fail 有 code", !((raw?["code"] as? String) ?? "").isEmpty)
            check("\(ln) fail 有 message", !f.message.isEmpty)
        case .result(let r):
            check("\(ln) result 有 code", !r.code.rawValue.isEmpty)
        case .step(let s):
            check("\(ln) step 有 code", !s.code.rawValue.isEmpty)
        case .ask(let q):
            check("\(ln) ask 有序号和题号", q.seq > 0 && !q.q.isEmpty, q.q)
            check("\(ln) ask 的字段 App 都能显示", q.isDisplayable)
            check("\(ln) 同一页的字段 id 不重复", Set(q.fields.map(\.id)).count == q.fields.count)
            check("\(ln) 密文字段不带默认值和旧值", q.fields.allSatisfy { $0.kind != .secret || ($0.defaultValue.isEmpty && $0.value.isEmpty) })
        case .other:
            break
        }
    }
    check("\(name) 整份解析和逐行一致", EventLog.parse(d) == evs)
    knownEnums(name, auditEvents(evs))
    return evs
}

// MARK: 容错（两种模式都跑：改动文档后照样解码）

/// 给每个对象加不认识的字段（can_why、hello 的 tools 是字典：键就是数据，不往里加）。
func injectUnknown(_ x: Any, parentKey: String? = nil) -> Any {
    if var d = x as? [String: Any] {
        for (k, v) in d { d[k] = injectUnknown(v, parentKey: k) }
        if parentKey != "can_why" && parentKey != "tools" {
            d["zz_unknown_object"] = ["nested": [1, 2.5, "x", NSNull()] as [Any], "flag": true] as [String: Any]
            d["zz_unknown_text"] = "future"
        }
        return d
    }
    if let a = x as? [Any] { return a.map { injectUnknown($0, parentKey: parentKey == "tools" ? "tool" : nil) } }
    return x
}

private let enumKeys: Set<String> = ["login", "source", "error_code", "result", "state", "sync", "local_sync", "route", "route_pref",
                                     "client", "backend", "dns", "webrtc", "severity", "default", "item", "plan_tier", "basis"]

/// 把枚举字段换成不认识的值（顶层的 kind 不动）。
func futureEnums(_ x: Any, depth: Int = 0) -> Any {
    if var d = x as? [String: Any] {
        for (k, v) in d {
            if v is String && (enumKeys.contains(k) || (depth > 0 && (k == "kind" || k == "code"))) {
                d[k] = "zz_future_value"
            } else {
                d[k] = futureEnums(v, depth: depth + 1)
            }
        }
        return d
    }
    if let a = x as? [Any] { return a.map { futureEnums($0, depth: depth + 1) } }
    return x
}

/// 类型不对的字段：数字字段给字符串，布尔字段给字符串，字符串字段给数字。
func wrongTypes(_ x: Any) -> Any {
    let numeric: Set<String> = ["port", "pid", "trust", "age_seconds", "utilization", "min_trust", "ssh_port", "availableCount",
                                "sessions", "since_epoch", "last_used_epoch", "seen_epoch", "left_percent", "from_epoch", "until_epoch",
                                "usd", "requests", "unpriced_requests", "weeks_usd", "days_usd", "synced_epoch", "average_usd", "files"]
    let boolean: Set<String> = ["stale", "pinned", "is_last", "running", "applies", "reset", "reachable", "hook", "cron", "listening", "saved",
                                "me", "revoked", "partial", "current", "first"]
    let text: Set<String> = ["plan", "plan_tier", "label", "message", "org", "trust_label", "balance", "error", "local_node", "selected",
                             "currency", "prices_date", "date"]
    if var d = x as? [String: Any] {
        for (k, v) in d {
            if v is [String: Any] {   // 同一个名字的对象（cost_total 的 me，设备的 me 是布尔）不是这里说的字段
                d[k] = wrongTypes(v)
            } else if numeric.contains(k) {
                d[k] = "abc"
            } else if boolean.contains(k) {
                d[k] = "yes"
            } else if text.contains(k) {
                d[k] = 42
            } else {
                d[k] = wrongTypes(v)
            }
        }
        return d
    }
    if let a = x as? [Any] { return a.map(wrongTypes) }
    return x
}

/// 文本层面把整数写成浮点数（7901 → 7901.0）、把 .0 结尾的浮点数写成整数（字符串里的不动）。
func flipNumbers(_ text: String) -> String {
    var out = ""
    var i = text.startIndex
    var inString = false
    while i < text.endIndex {
        let c = text[i]
        if inString {
            out.append(c)
            if c == "\\" {
                i = text.index(after: i)
                if i < text.endIndex { out.append(text[i]) }
            } else if c == "\"" {
                inString = false
            }
            i = text.index(after: i)
            continue
        }
        if c == "\"" {
            inString = true
            out.append(c)
            i = text.index(after: i)
            continue
        }
        if c == "-" || c.isNumber {
            var j = i
            while j < text.endIndex, "-+0123456789.eE".contains(text[j]) { j = text.index(after: j) }
            let tok = String(text[i..<j])
            if tok.range(of: #"^-?\d+\.0+$"#, options: .regularExpression) != nil {
                out += String(tok[..<tok.firstIndex(of: ".")!])
            } else if tok.range(of: #"^-?\d+$"#, options: .regularExpression) != nil {
                out += tok + ".0"
            } else {
                out += tok
            }
            i = j
            continue
        }
        out.append(c)
        i = text.index(after: i)
    }
    return out
}

func tolerance<T: CcaDocument & Equatable>(_ type: T.Type, _ name: String, _ d: Data, counts: (T) -> [Int]) {
    guard let orig = try? DocumentDecoder.decode(T.self, from: d), let obj = jsonObject(d) else { return }
    if let d1 = try? JSONSerialization.data(withJSONObject: injectUnknown(obj)) {
        let v1 = try? DocumentDecoder.decode(T.self, from: d1)
        check("\(name) 加了不认识的字段照样解码、结果不变", v1 == orig)
    }
    let v2 = try? DocumentDecoder.decode(T.self, from: Data(flipNumbers(String(decoding: d, as: UTF8.self)).utf8))
    check("\(name) 整数、浮点数互换结果不变", v2 == orig)
    if let d3 = try? JSONSerialization.data(withJSONObject: futureEnums(obj)) {
        let v3 = try? DocumentDecoder.decode(T.self, from: d3)
        check("\(name) 不认识的枚举值照样解码", v3 != nil)
        if let v3 { check("\(name) 不认识的枚举值不丢元素", counts(v3) == counts(orig), "\(counts(v3)) vs \(counts(orig))") }
    }
    if let d4 = try? JSONSerialization.data(withJSONObject: wrongTypes(obj)) {
        let v4 = try? DocumentDecoder.decode(T.self, from: d4)
        check("\(name) 类型不对的字段照样解码", v4 != nil)
        if let v4 { check("\(name) 类型不对的字段不丢元素", counts(v4) == counts(orig), "\(counts(v4)) vs \(counts(orig))") }
    }
}

func eventsTolerance(_ name: String, _ d: Data) {
    let orig = EventLog.parse(d)
    var lines = String(decoding: d, as: UTF8.self).split(separator: "\n").map(String.init)
    lines.insert("not json at all", at: 0)
    lines.append(#"{"v":1,"ev":"zz_future_event","code":"x","payload":{"a":[1,2]}}"#)
    lines.append(#"{"v":1,"ev":"step","code":"zz_future_step","extra":{"a":1}}"#)
    lines.append("{")
    lines.append("")
    let evs = EventLog.parse(Data(lines.joined(separator: "\n").utf8))
    check("\(name) 坏行跳过、新事件类型收成 other", evs.count == orig.count + 2 && Array(evs.prefix(orig.count)) == orig)
    check("\(name) 不认识的事件类型", evs.dropLast().last == .other("zz_future_event"))
    check("\(name) 不认识的 step code", evs.last?.step?.code.rawValue == "zz_future_step" && evs.last?.step?.code.isKnown == false)
    var same = true
    for line in String(decoding: d, as: UTF8.self).split(separator: "\n") {
        guard var obj = jsonObject(Data(line.utf8)) as? [String: Any] else { continue }
        let before = EventLog.parseLine(Data(line.utf8))
        obj["zz_unknown"] = ["x": 1]
        obj["zz_more"] = "future"
        let after = (try? JSONSerialization.data(withJSONObject: obj)).flatMap { EventLog.parseLine($0) }
        if before != after { same = false }
    }
    check("\(name) 事件加了不认识的字段结果不变", same)
}

func stateCounts(_ s: StateDoc) -> [Int] {
    let costs = s.tools.flatMap(\.accounts).compactMap(\.cost)
    return [s.tools.count, s.routing?.slots.count ?? -1, s.routing?.nodes.count ?? -1, s.notices.count, s.errors.count]
        + s.tools.map(\.accounts.count)
        + [costs.count, costs.map(\.weeks.count).reduce(0, +), costs.map(\.days.count).reduce(0, +), costs.map(\.devices.count).reduce(0, +),
           s.costTotal?.all.days.count ?? -1, s.costTotal?.all.byTool.count ?? -1, s.costTotal?.me?.days.count ?? -1, s.costTotal?.me?.byTool.count ?? -1]
}

/// 目录里每个文件按 kind（usage 旧版没有 kind 时看文件名）分派。
func runGeneric() -> (hello: [String: HelloDoc], state: [String: StateDoc]) {
    var hellos: [String: HelloDoc] = [:]
    var states: [String: StateDoc] = [:]
    let files = ((try? FileManager.default.contentsOfDirectory(atPath: fixtureDir.path)) ?? []).sorted()
    check("目录里有文件", !files.isEmpty, fixtureDir.path)
    for f in files where f.hasSuffix(".json") {
        section(f)
        guard let d = fixture(f) else { continue }
        let env = try? JSONDecoder().decode(Envelope.self, from: d)
        let kind = env?.kind ?? (f.hasPrefix("usage") ? "usage" : "?")
        switch kind {
        case "hello":
            if let h = genericHello(f, d) { hellos[f] = h }
            tolerance(HelloDoc.self, f, d) { [$0.tools.count] }
        case "state":
            if let s = genericState(f, d) { states[f] = s }
            tolerance(StateDoc.self, f, d, counts: stateCounts)
        case "live":
            genericLive(f, d)
            tolerance(LiveDoc.self, f, d) { [$0.slots.count] }
        case "usage":
            genericUsage(f, d)
            tolerance(UsageDoc.self, f, d) { [$0.accounts.count, $0.accounts.reduce(0) { $0 + $1.modelScoped.count }] }
        case "check":
            genericCheck(f, d)
            tolerance(CheckDoc.self, f, d) { [$0.accounts.count, $0.sharedExitIPs.count] }
        case "reset":
            genericReset(f, d)
            tolerance(ResetDoc.self, f, d) { [$0.usage == nil ? 0 : 1, $0.usage?.modelScoped.count ?? 0] }
        case "policy_pull":
            if let doc = decodeOrFail(PolicyPullDoc.self, f, d) {
                check("\(f) 有说明", !doc.summary.isEmpty)
            }
            tolerance(PolicyPullDoc.self, f, d) { [$0.changed ? 1 : 0] }
        case "devices":
            genericDevices(f, d)
            tolerance(DevicesDoc.self, f, d) { [$0.accounts.count, $0.accounts.reduce(0) { $0 + $1.devices.count }] }
        case "update":
            genericUpdate(f, d)
            tolerance(UpdateDoc.self, f, d) { [$0.latest == nil ? 0 : 1] }
        case "cost_sync":
            genericCostSync(f, d)
            tolerance(CostSyncDoc.self, f, d) { [$0.scan == nil ? 0 : 1, $0.sync == nil ? 0 : 1] }
        case "client_updates":
            if let doc = decodeOrFail(ClientUpdatesDoc.self, f, d) {
                check("\(f) version 1", doc.version == 1)
                check("\(f) 客户端 ID 不为空且不重复", doc.clients.allSatisfy { !$0.id.isEmpty }
                      && Set(doc.clients.map(\.id)).count == doc.clients.count)
                check("\(f) 可安装时有绑定检查结果的 token", doc.clients.allSatisfy { !$0.canInstall || nonEmpty($0.installToken) != nil })
                check("\(f) 允许自动更新只在自动检查开着时；等客户端退出的都能装", (doc.autoInstall != true || doc.mode == "auto")
                      && doc.clients.allSatisfy { !$0.autoInstallPending || ($0.canInstall && doc.autoInstall == true) })
            }
            tolerance(ClientUpdatesDoc.self, f, d) { [$0.clients.count] }
        default:
            check("\(f) 是认识的文档", false, "kind=\(kind)")
        }
    }
    for f in files where f.hasSuffix(".jsonl") {
        section(f)
        guard let d = fixture(f) else { continue }
        genericEvents(f, d)
        eventsTolerance(f, d)
    }
    return (hellos, states)
}

// MARK: 枚举值都认识（防漂移：cca 加了新值而 App 的已知列表没跟上时，这里先报出来）

final class EnumAudit {
    private(set) var unknown: [String] = []

    func add<E: KnownValues>(_ path: String, _ v: E?) {
        if let v, !v.isKnown { unknown.append("\(path)=\(v.rawValue)") }
    }

    func health(_ p: String, _ h: Health?) {
        guard let h else { return }
        add(p + ".source", h.source)
        add(p + ".kind", h.kind)
        add(p + ".route", h.route)
        add(p + ".result", h.result)
        add(p + ".leak.dns", h.leak?.dns)
        add(p + ".leak.webrtc", h.leak?.webrtc)
    }

    func usage(_ p: String, _ u: UsageEntry?) {
        guard let u else { return }
        add(p + ".source", u.source)
        add(p + ".error_code", u.errorCode)
    }
}

func knownEnums(_ name: String, _ unknown: [String]) {
    check("\(name) 枚举值都认识", unknown.isEmpty, unknown.joined(separator: ", "))
}

func auditHello(_ h: HelloDoc) -> [String] {
    let a = EnumAudit()
    for t in h.tools.keys { a.add("tools", t) }
    return a.unknown
}

func auditState(_ s: StateDoc) -> [String] {
    let a = EnumAudit()
    for (i, e) in s.errors.enumerated() {
        a.add("errors[\(i)].code", e.code)
        a.add("errors[\(i)].tool", e.tool)
    }
    for (i, n) in s.notices.enumerated() {
        a.add("notices[\(i)].code", n.code)
        a.add("notices[\(i)].tool", n.tool)
    }
    a.add("config.route_pref", s.config?.routePref)
    a.add("config.local.client", s.config?.local?.client)
    if let r = s.routing {
        a.add("routing.sync", r.sync)
        a.add("routing.local_sync", r.localSync)
        for sl in r.slots { a.add("slots[\(sl.slot)].tool", sl.tool) }
        for n in r.nodes { for u in n.users { a.add("nodes[\(n.name)].users.tool", u.tool) } }
    }
    for t in s.tools {
        a.add("tools.id", t.id)
        for (i, e) in t.errors.enumerated() {
            a.add("tools[\(t.id)].errors[\(i)].code", e.code)
            a.add("tools[\(t.id)].errors[\(i)].tool", e.tool)
        }
        for acc in t.accounts {
            let p = "\(t.id.rawValue)/\(acc.key)"
            a.add(p + ".kind", acc.kind)
            a.add(p + ".login", acc.login)
            a.add(p + ".plan_tier", acc.planTier)
            for (k, w) in acc.canWhy { a.add(p + ".can_why." + k, w) }
            a.usage(p + ".usage", acc.usage)
            a.health(p + ".health", acc.health)
            if let c = acc.cost {
                for (i, w) in c.weeks.enumerated() { a.add(p + ".cost.weeks[\(i)].basis", w.basis) }
                a.add(p + ".cost.sync.state", c.sync?.state)
            }
        }
    }
    a.add("cost_total.scan.state", s.costTotal?.scan.state)
    return a.unknown
}

func auditLive(_ l: LiveDoc) -> [String] {
    let a = EnumAudit()
    a.add("route", l.route)
    a.add("route_pref", l.routePref)
    a.add("router.backend", l.router?.backend)
    a.add("router.error.code", l.router?.error?.code)
    a.add("local.client", l.local?.client)
    a.add("local.error.code", l.local?.error?.code)
    for s in l.slots {
        a.add("slots[\(s.slot)].tool", s.tool)
        a.add("slots[\(s.slot)].state", s.state)
    }
    return a.unknown
}

func auditUsage(_ u: UsageDoc) -> [String] {
    let a = EnumAudit()
    a.add("tool", u.tool)
    for e in u.accounts { a.usage("[\(e.key ?? "?")]", e) }
    return a.unknown
}

func auditDevices(_ doc: DevicesDoc) -> [String] {
    let a = EnumAudit()
    a.add("tool", doc.tool)
    for acc in doc.accounts { for dev in acc.devices { a.add("[\(acc.key)].\(dev.name).state", dev.state) } }
    return a.unknown
}

func auditCheck(_ c: CheckDoc) -> [String] {
    let a = EnumAudit()
    a.add("tool", c.tool)
    a.add("route", c.route)
    for acc in c.accounts { a.health("[\(acc.key)].health", acc.health) }
    return a.unknown
}

func auditEvents(_ evs: [CcaEvent]) -> [String] {
    let a = EnumAudit()
    for (i, e) in evs.enumerated() {
        let p = "[\(i + 1)]"
        a.add(p + ".ev", e.type)
        switch e {
        case .step(let x): a.add(p + ".code", x.code)
        case .consent(let x):
            a.add(p + ".item", x.item)
            a.add(p + ".severity", x.severity)
        case .terminal(let x): a.add(p + ".code", x.code)
        case .fail(let x): a.add(p + ".code", x.code)
        case .result(let x):
            a.add(p + ".code", x.code)
            a.add(p + ".route", x.route)
        case .ask(let x):
            for f in x.fields {
                a.add(p + ".\(f.id).kind", f.kind)
                a.add(p + ".\(f.id).severity", f.severity)
            }
        case .other: break
        }
    }
    return a.unknown
}
