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
            // 本周的窗口：Claude、Codex 是 seven_day；agy 是各组的本周额度里用得最少的那个（cca 的 agy_week，同一条规则）
            let week = u.pressureWindows(now: uf.fromEpoch).first { $0.slot == .sevenDay }?.window
            check("\(n) use_first 区间到本周的重置时间为止", uf.fromEpoch < uf.untilEpoch && uf.untilEpoch == week?.resetsEpoch,
                  "\(uf) \(String(describing: week?.resetsEpoch))")
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

/// 按模型的各项按 id（模型名；没按模型记下的那一项）加起来：列出的周期的金额、请求数、没有公开价格的请求数，30 天的同样三项
private func modelTotals(_ lists: [[CostModelRow]]) -> [String: [Double]] {
    var out: [String: [Double]] = [:]
    for rows in lists {
        for r in rows {
            let v = [r.weeks.usd, Double(r.weeks.requests), Double(r.weeks.unpricedRequests),
                     r.days.usd, Double(r.days.requests), Double(r.days.unpricedRequests)]
            out[r.id] = zip(out[r.id] ?? Array(repeating: 0, count: 6), v).map { $0 + $1 }
        }
    }
    return out
}

/// rows 是不是 parts 按模型加起来的（金额差在四舍五入以内，请求数一样）
private func sameModels(_ rows: [CostModelRow], _ parts: [[CostModelRow]]) -> Bool {
    let a = modelTotals([rows]), b = modelTotals(parts), zero = Array(repeating: 0.0, count: 6)
    return Set(a.keys).union(b.keys).allSatisfy { k in
        let x = a[k] ?? zero, y = b[k] ?? zero
        return near(x[0], y[0], terms: parts.count + 1) && near(x[3], y[3], terms: parts.count + 1)
            && x[1] == y[1] && x[2] == y[2] && x[4] == y[4] && x[5] == y[5]
    }
}

/// 缓存命中率（PROTOCOL 2.2，3.7.5 起；旧版 cca 没有）：没有、null，或者 0～1 的数
private func hitValue(_ x: Any?) -> Bool {
    guard let x, !(x is NSNull) else { return true }
    guard isNumber(x), let v = (x as? NSNumber)?.doubleValue else { return false }
    return v >= 0 && v <= 1
}

/// 照 token 算的缓存命中率：读缓存的 ÷（没读缓存的输入 + 读缓存 + 写缓存）；1 只给全读缓存的，别的最多 0.9999；没有输入是 nil
private func expectedHit(_ list: [CostTokens]) -> Double? {
    let read = list.map(\.cacheRead).reduce(0, +), all = list.map { $0.input + $0.cacheRead + $0.cacheWrite }.reduce(0, +)
    guard all > 0 else { return nil }
    return read >= all ? 1 : min(Double(read) / Double(all), 0.9999)
}

/// cca 给的缓存命中率（四位小数）和照 token 算的一样
private func sameHit(_ hit: Double?, _ list: [CostTokens]) -> Bool {
    guard let e = expectedHit(list) else { return hit == nil }
    guard let hit else { return false }
    return e == 1 ? hit == 1 : abs(hit - e) <= 0.00006
}

/// 几部分加起来算的缓存命中率（按输入加权的平均）在各部分的之间；各部分都没有时它也没有
private func hitBetween(_ hit: Double?, _ parts: [Double?]) -> Bool {
    let p = parts.compactMap { $0 }
    guard let lo = p.min(), let hi = p.max() else { return hit == nil }
    guard let hit else { return false }
    return hit >= lo - 0.00011 && hit <= hi + 0.00011
}

/// 按模型分开的（PROTOCOL 2.2 的 `models[]`，3.6.8 起）：每一项的样子、排法；各项加起来是 s 的金额（withRequests：还有请求数）。
/// tokens：账号的带，设备、合计的不带
private func modelInvariants(_ n: String, _ raw: Any?, _ rows: [CostModelRow]?, summary s: CostSummary, withRequests: Bool, tokens: Bool) {
    guard let list = raw as? [[String: Any]], let rows else {
        check("\(n) 是对象数组", false)
        return
    }
    check("\(n) 每一项都解码了，model 是字符串或 null", list.count == rows.count && list.allSatisfy { $0["model"] is String || $0["model"] is NSNull },
          "\(list.count) vs \(rows.count)")
    let named = rows.filter { $0.model != nil }
    check("\(n) 模型名不重复，没按模型记下的最多一项、在最后", Set(named.map(\.id)).count == named.count && rows.dropLast().allSatisfy { $0.model != nil })
    let ranges = list.flatMap { r in ["weeks", "days"].map { r[$0] as? [String: Any] } }
    check("\(n) weeks、days 的金额、请求数不小于 0", ranges.allSatisfy { r in
        guard let r else { return false }
        return nonNegative(r["usd"]) && wholeCount(r["requests"]) && wholeCount(r["unpriced_requests"])
    })
    check("\(n) " + (tokens ? "tokens 是四个不小于 0 的整数" : "没有 tokens"), ranges.allSatisfy { r in
        guard let t = r?["tokens"] else { return !tokens }
        guard tokens, let o = t as? [String: Any] else { return false }
        return ["input", "output", "cache_read", "cache_write"].allSatisfy { wholeCount(o[$0]) }
    })
    check("\(n) cache_hit（3.7.5 起）没有、null 或者 0～1", ranges.allSatisfy { hitValue($0?["cache_hit"]) })
    if tokens {
        check("\(n) cache_hit 是这一段的 token 算的", zip(list, rows).allSatisfy { r, row in
            [("weeks", row.weeks), ("days", row.days)].allSatisfy { k, x in
                guard (r[k] as? [String: Any])?["cache_hit"] != nil, let t = x.tokens else { return true }
                return sameHit(x.cacheHit, [t])
            }
        })
    }
    check("\(n) priced_as（3.6.14 起）没有，或者是模型名（只在有名字的模型上）", list.allSatisfy { r in
        guard let p = r["priced_as"] else { return true }
        return (p as? String).map { !$0.isEmpty } == true && r["model"] is String
    })
    check("\(n) 没有公开价格的请求不多于请求数", rows.allSatisfy {
        $0.weeks.unpricedRequests <= $0.weeks.requests && $0.days.unpricedRequests <= $0.days.requests
    })
    check("\(n) 两段里都没有用量的不列", rows.allSatisfy { !$0.weeks.isEmpty || !$0.days.isEmpty })
    check("\(n) 按列出的周期的金额从多到少", zip(named, named.dropFirst()).allSatisfy { $0.weeks.usd >= $1.weeks.usd })
    check("\(n) 各项加起来是列出的周期、30 天的金额", near(rows.map(\.weeks.usd).reduce(0, +), s.weeksUSD, terms: rows.count + 1)
          && near(rows.map(\.days.usd).reduce(0, +), s.daysUSD, terms: rows.count + 1),
          "\(rows.map(\.weeks.usd).reduce(0, +)) vs \(s.weeksUSD), \(rows.map(\.days.usd).reduce(0, +)) vs \(s.daysUSD)")
    if withRequests {
        check("\(n) 各项的请求数加起来是 summary 的", rows.map(\.weeks.requests).reduce(0, +) == s.requests
              && rows.map(\.weeks.unpricedRequests).reduce(0, +) == s.unpricedRequests,
              "\(rows.map(\.weeks.requests).reduce(0, +)) vs \(s.requests)")
    }
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
    // 缓存命中率（3.7.5 起；旧版 cca 没有）：列出的周期、30 天的 token 加起来算的
    let rawSummary = raw["summary"] as? [String: Any] ?? [:]
    check("\(n) weeks_cache_hit、days_cache_hit 没有、null 或者 0～1", hitValue(rawSummary["weeks_cache_hit"]) && hitValue(rawSummary["days_cache_hit"]))
    if rawSummary["weeks_cache_hit"] != nil, w.allSatisfy({ $0.tokens != nil }) {
        check("\(n) weeks_cache_hit 是列出的周期的 token 算的", sameHit(s.weeksCacheHit, w.compactMap(\.tokens)), "\(String(describing: s.weeksCacheHit))")
    }
    if rawSummary["days_cache_hit"] != nil, d.allSatisfy({ $0.tokens != nil }) {
        check("\(n) days_cache_hit 是 30 天的 token 算的", sameHit(s.daysCacheHit, d.compactMap(\.tokens)), "\(String(describing: s.daysCacheHit))")
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
              && (nonNegative(ds["average_usd"]) || isNull(ds["average_usd"])) && (nonNegative(ds["average_day_usd"]) || isNull(ds["average_day_usd"]))
              && hitValue(ds["weeks_cache_hit"]) && hitValue(ds["days_cache_hit"]))
    }
    check("\(n) 每一项是各台设备加起来", w.indices.allSatisfy { i in
        near(c.devices.map { $0.weeksUSD[i] }.reduce(0, +), w[i].usd, terms: c.devices.count + 1)
    } && d.indices.allSatisfy { i in near(c.devices.map { $0.daysUSD[i] }.reduce(0, +), d[i].usd, terms: c.devices.count + 1) })
    check("\(n) 缓存命中率在各台设备的之间（整个账号的 token 是各台加起来的），都没有时没有",
          hitBetween(s.weeksCacheHit, c.devices.map(\.summary.weeksCacheHit)) && hitBetween(s.daysCacheHit, c.devices.map(\.summary.daysCacheHit)),
          "\(String(describing: s.weeksCacheHit)) \(String(describing: s.daysCacheHit))")
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
    // 按模型（3.6.8 起；旧版 cca 没有）：账号的带 tokens；每台设备都有自己的（没有 tokens），加起来是账号的
    if raw["models"] != nil {
        modelInvariants(n + " models", raw["models"], c.models, summary: s, withRequests: true, tokens: true)
        check("\(n) 有 models 时每台设备都有", (rawDevices ?? []).allSatisfy { $0["models"] != nil } && c.devices.allSatisfy { $0.models != nil })
        var parts: [[CostModelRow]] = []
        for (j, rd) in (rawDevices ?? []).enumerated() where j < c.devices.count && rd["models"] != nil {
            let dv = c.devices[j]
            modelInvariants("\(n) \(dv.name) models", rd["models"], dv.models, summary: dv.summary, withRequests: false, tokens: false)
            parts.append(dv.models ?? [])
        }
        check("\(n) models 是各台设备的加起来", sameModels(c.models ?? [], parts))
    }
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
        check("\(label) 缓存命中率（3.7.5 起）没有、null 或者 0～1", hitValue(raw["weeks_cache_hit"]) && hitValue(raw["days_cache_hit"])
              && ((raw["by_tool"] as? [String: Any]) ?? [:]).values.allSatisfy { x in
                  let o = x as? [String: Any] ?? [:]
                  return hitValue(o["weeks_cache_hit"]) && hitValue(o["days_cache_hit"])
              })
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
            check("\(label) 缓存命中率在各账号的之间（token 加起来算的），都没有时没有", hitBetween(s.weeksCacheHit, list.map(\.weeksCacheHit))
                  && hitBetween(s.daysCacheHit, list.map(\.daysCacheHit)), "\(String(describing: s.weeksCacheHit)) \(String(describing: s.daysCacheHit))")
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
    /// 按模型（3.6.8 起；旧版 cca 没有）：各项加起来是这一份的金额，是 parts（各账号的那一份）按模型加起来的
    func models(_ label: String, _ raw: [String: Any], _ p: CostTotalPart, _ parts: [[CostModelRow]]) {
        guard raw["models"] != nil else { return }
        modelInvariants(label + " models", raw["models"], p.models, summary: p.summary, withRequests: false, tokens: false)
        check("\(label) models 是各账号按模型加起来", sameModels(p.models ?? [], parts))
    }
    part(n, raw, t.all, accounts.compactMap { a in a.cost.map { (a.tool, $0.summary, $0.days) } })
    models(n, raw, t.all, accounts.compactMap { $0.cost?.models })
    // me：各账号里这台设备的那一份加起来（这台设备没有记录的账号不算）；按工具分开的工具和顶层一样
    guard let rawMe = raw["me"] as? [String: Any], let me = t.me else {
        check("\(n) 有 me", false)
        return
    }
    /// 一个账号里一台设备的 30 天（日期是账号的，金额是那台设备的 days_usd）
    func deviceDays(_ c: AccountCost, _ d: CostDevice) -> [CostPeriod] {
        c.days.enumerated().map { i, p in
            CostPeriod(startEpoch: p.startEpoch, endEpoch: p.endEpoch, date: p.date, current: p.current, usd: i < d.daysUSD.count ? d.daysUSD[i] : 0)
        }
    }
    part(n + " me", rawMe, me, accounts.compactMap { a in a.cost.flatMap { c in c.me.map { d in (a.tool, d.summary, deviceDays(c, d)) } } })
    models(n + " me", rawMe, me, accounts.compactMap { $0.cost?.me?.models })
    check("\(n) me 和顶层的工具一样、天一样", Set(me.byTool.keys) == Set(t.all.byTool.keys) && me.days.map(\.startEpoch) == t.all.days.map(\.startEpoch))
    check("\(n) 这台设备的不比所有设备的多", me.summary.weeksUSD <= t.all.summary.weeksUSD + 0.01 && me.summary.daysUSD <= t.all.summary.daysUSD + 0.01
          && zip(me.days, t.all.days).allSatisfy { $0.usd <= $1.usd + 0.01 })
    // devices：按设备分开加起来（旧版 cca 没有，是空的）。这台设备在第一项、和 me 一样；别的设备按 id 归到一起（只算账号里 used 的那一份），
    // 按 30 天的金额从多到少；用过账号的设备都在里面，各台加起来是顶层的
    if raw["devices"] != nil {
        let rawDevices = raw["devices"] as? [[String: Any]]
        check("\(n) devices 是对象数组，每一台都解码了", rawDevices?.count == t.devices.count, "\(rawDevices?.count ?? -1) vs \(t.devices.count)")
        check("\(n) devices 的 id 不重复，这台设备在第一项、只有它是 me", Set(t.devices.map(\.id)).count == t.devices.count
              && t.devices.first?.isMe == true && t.devices.filter(\.isMe).count == 1, t.devices.map(\.name).joined(separator: ","))
        if let first = t.devices.first {
            let mine = accounts.compactMap { $0.cost?.me }
            check("\(n) devices 的这台设备：和 me 一样，id 和账号里的一样", first.part == me
                  && mine.allSatisfy { $0.id == first.id } && !first.revoked && !first.stale, first.id)
        }
        let others = Array(t.devices.dropFirst())
        check("\(n) 别的设备按 30 天的金额从多到少", zip(others, others.dropFirst()).allSatisfy { $0.part.summary.daysUSD >= $1.part.summary.daysUSD - 0.001 })
        for (i, d) in t.devices.enumerated() where !d.isMe {
            let shares = accounts.compactMap { a in a.cost.flatMap { c in c.device(d.id).flatMap { x in x.used ? (a.tool, c, x) : nil } } }
            part("\(n) devices[\(i)]", rawDevices?[i] ?? [:], d.part, shares.map { ($0.0, $0.2.summary, deviceDays($0.1, $0.2)) })
            models("\(n) devices[\(i)]", rawDevices?[i] ?? [:], d.part, shares.map { $0.2.models ?? [] })
            if shares.isEmpty, let o = t.unattributed?.cost?.device(d.id) {
                // 3.6.8 起：只有认不出是哪个账号的用量的设备（账号的几项都是 0）
                check("\(n) devices[\(i)] 只有认不出账号的用量：没撤销，很久没同步和它传来的那一份一样", !d.revoked && d.stale == o.stale, d.name)
            } else {
                check("\(n) devices[\(i)] 用过账号，撤销了（哪个账号里说了就算）、很久没同步（每个账号里都是）", !shares.isEmpty
                      && d.revoked == shares.contains { $0.2.revoked } && d.stale == shares.allSatisfy { $0.2.stale }, d.name)
            }
            check("\(n) devices[\(i)] 和顶层的工具一样、天一样", Set(d.part.byTool.keys) == Set(t.all.byTool.keys)
                  && d.part.days.map(\.startEpoch) == t.all.days.map(\.startEpoch))
        }
        let orphanOnly = t.devices.map { d in !d.isMe && !accounts.contains { $0.cost?.device(d.id)?.used == true } }
        check("\(n) 只有认不出账号的用量的设备排在最后", !zip(orphanOnly, orphanOnly.dropFirst()).contains { $0 && !$1 },
              t.devices.map(\.name).joined(separator: ","))
        let listed = Set(t.devices.map(\.id))
        check("\(n) 在账号里用过（used）的设备都在 devices 里", accounts.allSatisfy { a in a.cost?.devices.allSatisfy { !$0.used || listed.contains($0.id) } ?? true })
        let parts = t.devices.map(\.part.summary)
        check("\(n) 各台设备加起来是顶层的", near(t.all.summary.currentUSD, parts.map(\.currentUSD).reduce(0, +), terms: parts.count)
              && near(t.all.summary.weeksUSD, parts.map(\.weeksUSD).reduce(0, +), terms: parts.count)
              && near(t.all.summary.daysUSD, parts.map(\.daysUSD).reduce(0, +), terms: parts.count)
              && near(t.all.summary.todayUSD, parts.map(\.todayUSD).reduce(0, +), terms: parts.count), "\(parts.map(\.weeksUSD))")
    }
    let scan = raw["scan"] as? [String: Any]
    check("\(n) scan 的字段类型", scan?["state"] is String && (isNumber(scan?["updated_epoch"]) || isNull(scan?["updated_epoch"])))
    if t.scan.state == .ready { check("\(n) 统计过（ready）就有 updated_epoch", t.scan.updatedEpoch != nil) }
    if let u = raw["unattributed"], !isNull(u) {
        let o = u as? [String: Any]
        check("\(n) unattributed 是不小于 0 的金额和请求数", nonNegative(o?["days_usd"]) && wholeCount(o?["requests"]))
        // 3.6.9 起：app_hook 是 on / off，不是 macOS 时 null（旧版 cca 没有）
        check("\(n) unattributed.app_hook 是字符串或 null", o?["app_hook"] == nil || isNull(o?["app_hook"]) || o?["app_hook"] is String)
        // 3.6.8 起：所有设备的（和账号的 cost 一样的样子，按日历周切），设备菜单里还没传过它的设备
        if let rc = o?["cost"], !isNull(rc) {
            let c = t.unattributed?.cost
            costInvariants(n + " unattributed.cost", rc as? [String: Any] ?? [:], c, now: now)
            if let c {
                check("\(n) unattributed.cost 按日历周切，sync.missing 是空的", c.weeks.allSatisfy { $0.basis == .calendar }
                      && (c.sync?.missing.isEmpty ?? true))
                if raw["devices"] != nil {
                    let listed = Set(t.devices.map(\.id))
                    check("\(n) unattributed.cost 里的设备都在 devices 里（这台设备的 id 一样）", c.devices.allSatisfy { listed.contains($0.id) }
                          && (c.me.map { $0.id == t.devices.first?.id } ?? true), c.devices.map(\.name).joined(separator: ","))
                }
            }
        }
        if let ru = o?["unsynced"] {
            let list = ru as? [[String: Any]], ids = t.unattributed?.unsynced.map(\.id) ?? []
            check("\(n) unattributed.unsynced 是对象数组，都有 id、名字", list != nil && list?.count == ids.count && (list ?? []).allSatisfy {
                !(($0["id"] as? String) ?? "").isEmpty && !(($0["name"] as? String) ?? "").isEmpty
            })
            let sent = Set(t.unattributed?.cost?.devices.map(\.id) ?? [])
            check("\(n) unsynced 是设备菜单里别的设备，不重复，不在 unattributed.cost 里", Set(ids).count == ids.count && ids.allSatisfy { id in
                !sent.contains(id) && (raw["devices"] == nil || t.devices.contains { $0.id == id && !$0.isMe })
            }, ids.joined(separator: ","))
        }
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
    if let rawHubs = sync?["hubs"] {
        let rh = rawHubs as? [[String: Any]] ?? []
        check("\(name) sync.hubs 都解码了、各有 kind", rh.count == doc.sync?.hubs.count && doc.sync?.hubs.allSatisfy { $0.kind != nil } == true)
        if let s = doc.sync, !s.hubs.isEmpty {
            check("\(name) 顶层的 sync 是 hubs 里的一台的", s.hubs.contains { $0.state == s.state && $0.message == s.message })
        }
    }
    let a = EnumAudit()
    a.add("scan.state", doc.scan?.state)
    a.add("sync.state", doc.sync?.state)
    for (i, h) in (doc.sync?.hubs ?? []).enumerated() {
        a.add("sync.hubs[\(i)].kind", h.kind)
        a.add("sync.hubs[\(i)].state", h.state)
    }
    knownEnums(name, a.unknown)
    return doc
}

/// 3.6.0 起 `config.policy` 固定是这一份（PROTOCOL 2.2）：`{"configured": false, "repo": null, "next_check_epoch": null, "state": "off", "accounts": 0, "parked": 0}`
func isConstantPolicy(_ v: Any) -> Bool {
    guard let p = v as? [String: Any] else { return false }
    return ["configured", "repo", "next_check_epoch", "state", "accounts", "parked"].allSatisfy { p[$0] != nil }
        && isBool(p["configured"]) && p["configured"] as? Bool == false && isNull(p["repo"]) && isNull(p["next_check_epoch"])
        && p["state"] as? String == "off" && isNumber(p["accounts"]) && (p["accounts"] as? NSNumber)?.doubleValue == 0
        && isNumber(p["parked"]) && (p["parked"] as? NSNumber)?.doubleValue == 0
}

/// `config.hubs[]`（PROTOCOL 2.2）
func hubEntryInvariants(_ n: String, _ h: HubEntry, _ raw: [String: Any]) {
    check("\(n) 字段类型", raw["kind"] is String && raw["url"] is String && raw["host"] is String && raw["state"] is String
          && (raw["error"] is String || isNull(raw["error"])) && (isNumber(raw["checked_epoch"]) || isNull(raw["checked_epoch"]))
          && wholeCount(raw["accounts"]) && (raw["name"] is String || isNull(raw["name"]))
          && (raw["team"] is String || isNull(raw["team"])) && (raw["team_name"] is String || isNull(raw["team_name"])))
    check("\(n) 地址是 https，host 是它的主机和端口", h.url.hasPrefix("https://") && h.url.hasSuffix(h.host) && !h.host.isEmpty)
    check("\(n) 没同步成的有原因，别的没有", (h.state == .error) == (h.error != nil))
    check("\(n) 成了就有时间", h.state != .ok || h.checkedEpoch != nil)
    check("\(n) 是哪个 Team 的服务器：有 Team 的编号和名字", h.kind == .team && h.team?.count == 32 && h.teamName != nil)
    // 以前只有自己的同步服务器才有值的几项：接口只加不删，cca 照旧给空的（PROTOCOL 2.2）
    check("\(n) members、joined_epoch、pinned 是 null，can_invite 是 false", isNull(raw["members"]) && isNull(raw["joined_epoch"])
          && isNull(raw["pinned"]) && isBool(raw["can_invite"]) && raw["can_invite"] as? Bool == false)
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
    if let can = ra["can"] as? [String: Any], can["policy_lock"] != nil {
        // 请求维护者锁定节点（3.6.0 去掉了）：接口只加不删，cca 照旧给 false（PROTOCOL 2.2）
        check("\(n) can.policy_lock 总是 false", isBool(can["policy_lock"]) && can["policy_lock"] as? Bool == false)
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
                // 同步（PROTOCOL 2.2）：Team 给的官方账号、这台设备用得了那个 Team 的服务器（和「设备」一样），Antigravity 不同步；来源、时间只给记下的
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
    // 邮箱助手（PROTOCOL 2.2、4.11）：can.mail 只给 Claude 官方账号（有邮箱），有 mail 的账号 can.mail 为真，第三方 API 账号都没有
    if let can = ra["can"] as? [String: Any], can["mail"] != nil {
        check("\(n) can.mail 是布尔", isBool(can["mail"]))
        if a.can.mail { check("\(n) 有「邮箱」的是 Claude 官方账号、有邮箱", t.id == .claude && !a.isAPI && nonEmpty(a.email) != nil) }
    }
    if a.kind == .api {
        check("\(n) API 账号没有邮箱助手", a.mail == nil && isNull(ra["mail"]) && !a.can.mail)
    }
    if let raw = ra["mail"], !isNull(raw) {
        check("\(n) 有 mail 的账号 can.mail 为真", a.can.mail)
        check("\(n) mail.source 是 account / default / 空", [nil, "account", "default", ""].contains(a.mail?.source))
        // 信任转发中转（PROTOCOL 4.11）：forwarder 可缺省 / null / 字符串；有值时 trustsForwarder 为真（不安全开关）
        if let mraw = raw as? [String: Any] {
            check("\(n) mail.forwarder 是字符串或缺省", mraw["forwarder"] == nil || isNull(mraw["forwarder"]) || mraw["forwarder"] is String)
        }
        if let f = a.mail?.forwarder {
            check("\(n) mail.forwarder 非空、trustsForwarder 为真", !f.isEmpty && a.mail?.trustsForwarder == true)
        } else {
            check("\(n) 没登记中转时 trustsForwarder 为假", a.mail?.trustsForwarder == false)
        }
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
                  && ["cost", "usage_passive"].allSatisfy { f[$0] == nil || isBool(f[$0]) })
        }
        if t.id == .agy {
            check("\(name) agy 的 cli_prefix 是 [agy]", t.cliPrefix == ["agy"])
            // 3.6.10 及以前的 cca 给的 features.usage 是 false（查不了用量：unsupported、usage 是 null），以后的是 true
            check("\(name) agy 没有桌面 App、第三方 API 账号", t.features.app == false && t.features.api == false)
            check("\(name) agy 的账号都是官方账号，开不了 App，没有账单页、用量页、每周用量的统计",
                  t.accounts.allSatisfy { !$0.isAPI && !$0.can.app && $0.canWhy["app"] == .unsupported && !$0.can.billing && !$0.can.limitResets
                      && $0.usageWeeks == nil })
            check("\(name) agy 的用量：features.usage 是 false 时都查不了（unsupported）；是 true 时不说 unsupported，有的话是几组额度（没有 5 小时、本周这些窗口）",
                  t.features.usage
                      ? t.accounts.allSatisfy { $0.canWhy["usage"] != .unsupported && ($0.usage.map { $0.fiveHour == nil && $0.sevenDay == nil } ?? true) }
                      : t.accounts.allSatisfy { !$0.can.usage && $0.canWhy["usage"] == .unsupported && $0.usage == nil })
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
    // config.teams（PROTOCOL 2.2、4.14）：一个 Team 都不在时没有这个字段。账号的 team 是其中一个；第三方 API 账号不是谁给的
    let rawTeamConfig = raw["config"] as? [String: Any] ?? [:]
    let teams = doc.config?.teams ?? []
    if let rt = rawTeamConfig["teams"] {
        let rawTeams = (rt as? [[String: Any]] ?? []).filter { ($0["id"] as? String)?.isEmpty == false }
        check("\(name) config.teams 是对象数组，有编号的都解码了，编号不重复", rt is [Any] && rawTeams.count == teams.count
              && Set(teams.map(\.id)).count == teams.count)
    }
    for t in doc.tools {
        for a in t.accounts where a.team != nil {
            check("\(name) \(t.id.rawValue)/\(a.key) 是哪个 Team 给的：那个 Team 在 config.teams 里；不是第三方 API 账号",
                  teams.contains { $0.id == a.team } && !a.isAPI)
        }
    }
    // config.policy（PROTOCOL 2.2）：维护者的共享 3.6.0 去掉了，接口只加不删，cca 照旧给固定的「关着」那一份（已经发出去的 App 照它有没有
    // state 显示「共享与同步」页）
    let rawConfig = raw["config"] as? [String: Any] ?? [:]
    if let p = rawConfig["policy"] {
        check("\(name) config.policy 是固定的关着（configured false，state off，accounts、parked 是 0，repo、next_check_epoch 是 null）", isConstantPolicy(p), "\(p)")
    }
    // config.hubs（PROTOCOL 2.2）：旧版 cca 没有；有的话是各个 Team 的（每个 Team 最多一台，照编号分），账号走的那一台在里面
    if let rh = rawConfig["hubs"] {
        let rawHubs = rh as? [[String: Any]] ?? []
        let rawKnown = rawHubs.filter { HubKind(rawValue: $0["kind"] as? String ?? "").isKnown }
        check("\(name) config.hubs 是对象数组，认识的都解码了（不认识的 kind 去掉）", rh is [Any] && rawKnown.count == doc.config?.hubs.count)
        check("\(name) config.hubs 里没有维护者的服务器（3.6.0 去掉了）", !rawHubs.contains { $0["kind"] as? String == "maintainer" })
        let hubs = doc.config?.hubs ?? []
        check("\(name) config.hubs 每个 Team 最多一台（照编号算）", Set(hubs.map { $0.kind.rawValue + "/" + ($0.team ?? "") }).count == hubs.count
              && hubs.allSatisfy { ($0.kind == .team) == ($0.team?.isEmpty == false) })
        for (i, h) in hubs.enumerated() {
            hubEntryInvariants("\(name) config.hubs[\(i)]", h, i < rawKnown.count ? rawKnown[i] : [:])
        }
        for t in doc.tools {
            for a in t.accounts {
                check("\(name) \(t.id.rawValue)/\(a.key) 走的那台服务器在 config.hubs 里，有「设备」一节的才走",
                      a.hub == nil ? !a.can.devices : hubs.contains { $0.kind == a.hub && (a.hub != .team || $0.team == a.team) } && a.can.devices)
                check("\(name) \(t.id.rawValue)/\(a.key) 第三方 API 账号不经服务器同步", !a.isAPI || a.hub == nil)
                if let s = a.cost?.sync, s.state == .localOnly || s.state == .off {
                    check("\(name) \(t.id.rawValue)/\(a.key) 只在这台设备上的折合金额不说经哪台服务器", s.hub == nil)
                }
            }
        }
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
    if name.contains("agy") {
        check("\(name) Antigravity 的输出带 tool: agy，额度在 quota_groups 里、没有用量窗口", doc.tool == .agy
              && doc.accounts.allSatisfy { $0.fiveHour == nil && $0.sevenDay == nil && $0.modelScoped.isEmpty })
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
        if raw["hubs"] != nil {
            check("\(name) \(a.key) 问的那台服务器在 hubs 里（Team 的照编号认）", a.hub == nil || doc.hubs.contains { $0.kind == a.hub && $0.team == a.team })
        }
        for (j, dev) in a.devices.enumerated() {
            let r = j < rd.count ? rd[j] : [:]
            check("\(name) \(dev.name) 的时间是数字或 null", ["since_epoch", "last_active_epoch", "last_used_epoch", "seen_epoch"]
                .allSatisfy { isNull(r[$0]) || isNumber(r[$0]) })
            check("\(name) \(dev.name) 开着会话的才有开始时间", dev.state == .none || dev.state == .offline || dev.sinceEpoch != nil)
        }
    }
    if let rh = raw["hubs"] {
        check("\(name) hubs 都解码了，每类最多一台（Team 的照编号算）", (rh as? [Any])?.count == doc.hubs.count
              && Set(doc.hubs.map { $0.kind.rawValue + "/" + ($0.team ?? "") }).count == doc.hubs.count)
    }
    knownEnums(name, auditDevices(doc))
    return doc
}

/// `remote list --json`（PROTOCOL 9.1）：可以接入的会话、配对过的机器；没问到服务器的原因在 errors 里。
@discardableResult
func genericRemote(_ name: String, _ d: Data) -> RemoteDoc? {
    guard let doc = decodeOrFail(RemoteDoc.self, name, d) else { return nil }
    let raw = jsonObject(d) as? [String: Any] ?? [:]
    check("\(name) version 1", doc.version == 1)
    check("\(name) kind remote", doc.kind == "remote")
    check("\(name) generated_epoch 是数字", isNumber(raw["generated_epoch"]))
    check("\(name) enabled 是布尔", raw["enabled"] is Bool)
    let relay = raw["relay"] as? [String: Any]
    check("\(name) relay.available 是布尔，用不了时有原因", relay?["available"] is Bool && (doc.relayAvailable || nonEmpty(doc.relayMessage) != nil))
    if let rh = relay?["hubs"] {
        // 3.6.0 起只有各个 Team 的服务器（维护者的那一台去掉了：没有 team 的解不出来）
        check("\(name) relay.hubs 都解码了：都是某个 Team 的（编号 32 个十六进制字符、不重复）", (rh as? [Any])?.count == doc.relayHubs.count
              && doc.relayHubs.allSatisfy { $0.team.count == 32 && $0.team.allSatisfy(\.isHexDigit) } && Set(doc.relayHubs.map(\.team)).count == doc.relayHubs.count)
    }
    let rawSessions = raw["sessions"] as? [[String: Any]] ?? [], rawPeers = raw["peers"] as? [[String: Any]] ?? []
    check("\(name) 会话、配对过的机器、原因都解码了", rawSessions.count == doc.sessions.count && rawPeers.count == doc.peers.count
          && (raw["errors"] as? [[String: Any]])?.count == doc.errors.count)
    check("\(name) 会话的 id 是 32 个十六进制字符、不重复，short 是它的前 8 位",
          doc.sessions.allSatisfy { $0.id.count == 32 && $0.id.allSatisfy(\.isHexDigit) && $0.short == String($0.id.prefix(8)) }
          && Set(doc.sessions.map(\.id)).count == doc.sessions.count)
    check("\(name) 配对的 id 是 32 个十六进制字符、不重复", doc.peers.allSatisfy { $0.id.count == 32 && $0.id.allSatisfy(\.isHexDigit) }
          && Set(doc.peers.map(\.id)).count == doc.peers.count)
    for (i, s) in doc.sessions.enumerated() {
        let r = i < rawSessions.count ? rawSessions[i] : [:]
        check("\(name) \(s.short) 的 since_epoch 是数字或 null、viewers 是数字", (isNull(r["since_epoch"]) || isNumber(r["since_epoch"])) && isNumber(r["viewers"]))
        if s.isLocal {
            check("\(name) \(s.short) 这台 Mac 上的：没有 peer、machine，知道启动它的终端在不在，能看和操作",
                  isNull(r["peer"]) && isNull(r["machine"]) && r["local_terminal"] is Bool && s.access == .control && s.state != .offline)
        } else {
            check("\(name) \(s.short) 别的机器上的：peer 是配对过的机器，有机器名，不知道那边的终端，账号只有短名字（没有邮箱）",
                  doc.peers.contains { $0.id == s.peer } && nonEmpty(s.machine) != nil && isNull(r["local_terminal"]) && s.state != .busy
                  && !s.name.contains("@"))
        }
        check("\(name) \(s.short) 的 access 是 control 或 view", s.access == .control || s.access == .view)
        if let t = r["team"], !isNull(t) {
            // 会话属于哪个 Team：别的机器上的就是经它看到的那个配对记在的那一台
            check("\(name) \(s.short) 的 team 是 Team 的编号\(s.isLocal ? "" : "，和它的配对记在同一台上")", s.team?.count == 32
                  && (s.isLocal || doc.peers.first { $0.id == s.peer }?.team == nil || doc.peers.first { $0.id == s.peer }?.team == s.team))
        }
    }
    for (i, p) in doc.peers.enumerated() {
        let r = i < rawPeers.count ? rawPeers[i] : [:]
        check("\(name) \(p.short) 有名字，shown 是布尔，paired_epoch 是数字或 null",
              !p.name.isEmpty && r["shown"] is Bool && (isNull(r["paired_epoch"]) || isNumber(r["paired_epoch"])))
        if r["team"] != nil {
            // 3.6.0 起配对都记在某个 Team 的服务器上（以前经维护者的服务器配的，cca 读配对表时去掉了）
            check("\(name) \(p.short) 记在某个 Team 的服务器上", p.team?.count == 32)
        }
    }
    check("\(name) 用不了中转时不列别的机器上的会话", doc.relayAvailable || doc.sessions.allSatisfy(\.isLocal))
    knownEnums(name, auditRemote(doc))
    return doc
}

@discardableResult
func genericMailList(_ name: String, _ d: Data) -> MailListDoc? {
    guard let doc = decodeOrFail(MailListDoc.self, name, d) else { return nil }
    let raw = jsonObject(d) as? [String: Any] ?? [:]
    check("\(name) version 1", doc.version == 1)
    check("\(name) kind mail-list", doc.kind == "mail-list")
    check("\(name) generated_at 是数字", isNumber(raw["generated_at"]))
    let rawRows = raw["rows"] as? [[String: Any]] ?? []
    check("\(name) 有 error 时 rows 空", doc.error == nil || doc.rows.isEmpty)
    check("\(name) 行都解码了", rawRows.count == doc.rows.count)
    for row in doc.rows {
        // 通过核对的才有句柄和链接样子；没通过的说原因
        if row.verdict.ok {
            check("\(name) 通过核对的没有 why", row.verdict.why == nil)
        } else {
            check("\(name) 没通过核对的有原因", nonEmpty(row.verdict.whyText) != nil && nonEmpty(row.verdict.why) != nil)
            check("\(name) 没通过核对的不能打开、没有句柄", !row.openable && row.msg == nil)
        }
        // openable 必有句柄；句柄才是回传给 cca 的东西
        check("\(name) openable 的有句柄", !row.openable || row.msg != nil)
        // via（PROTOCOL 2.4）：nil = Anthropic 直签（安全）；"forwarder" = 经可信中转认的（不安全），只会落在能打开、通过核对的行上
        check("\(name) via 是 forwarder 或缺省", row.via == nil || row.via == "forwarder")
        if row.viaForwarder {
            check("\(name) 经可信中转的行通过核对、可打开", row.verdict.ok && row.openable)
        }
        // 绝不含真链接：只有 link_shape 这一个和链接有关的字段
        let r = rawRows.first { ($0["got_epoch"] as? NSNumber)?.doubleValue == row.gotEpoch }
        check("\(name) 行里只有 link_shape、没有真链接字段", r == nil || (r!["link"] == nil && r!["url"] == nil && r!["magic_link"] == nil))
        check("\(name) via 字段是 forwarder / null / 缺省", r == nil || r!["via"] == nil || isNull(r!["via"]) || (r!["via"] as? String) == "forwarder")
    }
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
/// 键是数据（邮箱、工具名）的那几个对象：往里塞一个键就成了一条数据，不塞（连它里面的也不塞）。只认值是对象的：别处同名的数组照塞
private let dataKeyedObjects: Set<String> = ["accounts", "aliases", "tool_aliases", "grants"]

func injectUnknown(_ x: Any, parentKey: String? = nil, inData: Bool = false) -> Any {
    if var d = x as? [String: Any] {
        for (k, v) in d { d[k] = injectUnknown(v, parentKey: k, inData: inData || (dataKeyedObjects.contains(k) && v is [String: Any])) }
        if parentKey != "can_why" && parentKey != "tools" && !inData {
            d["zz_unknown_object"] = ["nested": [1, 2.5, "x", NSNull()] as [Any], "flag": true] as [String: Any]
            d["zz_unknown_text"] = "future"
        }
        return d
    }
    if let a = x as? [Any] { return a.map { injectUnknown($0, parentKey: parentKey == "tools" ? "tool" : nil, inData: inData) } }
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
                                "me", "revoked", "partial", "current", "first", "alias_locked", "alias_lock"]
    let text: Set<String> = ["plan", "plan_tier", "label", "message", "org", "trust_label", "balance", "error", "local_node", "selected",
                             "currency", "prices_date", "date"]
    if var d = x as? [String: Any] {
        for (k, v) in d {
            if v is [String: Any] || v is [Any] {   // 同一个名字的对象、数组（cost_total 的 me，设备的 me 是布尔；远程会话的 sessions 是数组，cli 的是个数）不是这里说的字段
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
           s.costTotal?.all.days.count ?? -1, s.costTotal?.all.byTool.count ?? -1, s.costTotal?.me?.days.count ?? -1, s.costTotal?.me?.byTool.count ?? -1,
           costs.map { $0.models?.count ?? -1 }.reduce(0, +), s.costTotal?.all.models?.count ?? -1,
           s.costTotal?.unattributed?.cost?.devices.count ?? -1, s.costTotal?.unattributed?.unsynced.count ?? -1]
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
        case "devices":
            genericDevices(f, d)
            tolerance(DevicesDoc.self, f, d) { [$0.accounts.count, $0.accounts.reduce(0) { $0 + $1.devices.count }] }
        case "remote":
            genericRemote(f, d)
            tolerance(RemoteDoc.self, f, d) { [$0.sessions.count, $0.peers.count, $0.errors.count] }
        case "mail-list":
            genericMailList(f, d)
            tolerance(MailListDoc.self, f, d) { [$0.rows.count, $0.folders.count] }
        case "update":
            genericUpdate(f, d)
            tolerance(UpdateDoc.self, f, d) { [$0.latest == nil ? 0 : 1] }
        case "cost_sync":
            genericCostSync(f, d)
            tolerance(CostSyncDoc.self, f, d) { [$0.scan == nil ? 0 : 1, $0.sync == nil ? 0 : 1] }
        case "team_devices":
            // cca team devices <Team> --json（PROTOCOL 4.14）：设备编号不空不重复，这台设备最多一台，授权里的账号都在这个 Team 的配置里
            if let doc = decodeOrFail(TeamDevicesDoc.self, f, d) {
                check("\(f) version 1，有 Team 的编号", doc.version == 1 && doc.team.count == 32)
                check("\(f) 设备编号不为空且不重复，这台设备最多一台", doc.devices.allSatisfy { !$0.id.isEmpty }
                      && Set(doc.devices.map(\.id)).count == doc.devices.count && doc.devices.filter(\.me).count <= 1)
                let known = Set(doc.accountList.map(\.spec))
                check("\(f) 只给几个账号的设备：这些账号都在配置里", doc.devices.allSatisfy { dev in
                    dev.grantsAll || dev.grants.allSatisfy { tool, emails in emails.allSatisfy { known.contains("\(tool):\($0)") } }
                })
                check("\(f) 配置里的账号都有节点", doc.accountList.allSatisfy { !$0.node.isEmpty && !$0.email.isEmpty })
            }
            tolerance(TeamDevicesDoc.self, f, d) { [$0.devices.count, $0.accountList.count] }
        case "team_list":
            // cca team ls --json：teams[] 和 state 的 config.teams 一个样子
            if let obj = (try? JSONSerialization.jsonObject(with: d)) as? [String: Any], let rows = obj["teams"] as? [[String: Any]] {
                let teams = rows.compactMap { try? JSONDecoder().decode(TeamEntry.self, from: JSONSerialization.data(withJSONObject: $0)) }
                check("\(f) teams 都解码了，编号不空不重复", teams.count == rows.count && teams.allSatisfy { $0.id.count == 32 }
                      && Set(teams.map(\.id)).count == teams.count)
                check("\(f) 角色、状态都是认识的", teams.allSatisfy { $0.role.isKnown && $0.state.isKnown })
            } else {
                check("\(f) 有 teams 数组", false)
            }
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
    a.add("config.local.udp.state", s.config?.local?.udp?.state)
    // config.hubs[].kind、账号的 hub、cost.sync.hub 不认识的在解码时就去掉了（PROTOCOL：当作没有 / null），这里看不到
    for (i, h) in (s.config?.hubs ?? []).enumerated() {
        a.add("config.hubs[\(i)].state", h.state)
    }
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
    a.add("cost_total.unattributed.app_hook", s.costTotal?.unattributed?.appHook)
    if let c = s.costTotal?.unattributed?.cost {
        for (i, w) in c.weeks.enumerated() { a.add("cost_total.unattributed.cost.weeks[\(i)].basis", w.basis) }
        a.add("cost_total.unattributed.cost.sync.state", c.sync?.state)
    }
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
    for (i, h) in doc.hubs.enumerated() { a.add("hubs[\(i)].kind", h.kind) }
    for acc in doc.accounts {
        a.add("[\(acc.key)].hub", acc.hub)
        for dev in acc.devices { a.add("[\(acc.key)].\(dev.name).state", dev.state) }
    }
    return a.unknown
}

func auditRemote(_ doc: RemoteDoc) -> [String] {
    let a = EnumAudit()
    for s in doc.sessions {
        a.add("sessions[\(s.short)].where", s.place)
        a.add("sessions[\(s.short)].tool", s.tool)
        a.add("sessions[\(s.short)].state", s.state)
        a.add("sessions[\(s.short)].access", s.access)
    }
    for p in doc.peers {
        a.add("peers[\(p.short)].incoming", p.incoming)
        a.add("peers[\(p.short)].state", p.state)
    }
    for (i, e) in doc.errors.enumerated() { a.add("errors[\(i)].code", e.code) }
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
