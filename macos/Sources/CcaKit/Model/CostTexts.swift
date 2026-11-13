import Foundation

// 折合金额（cca 给的 cost、cost_total，PROTOCOL 2.2）的文字：金额、token 数、日期和提示。数字都照 cca 给的，不重算平均、合计。

extension Texts {
    // MARK: 数字

    /// 金额：0 写 `$0`，不到一分写 `<$0.01`，不到 100 带两位小数（`$12.35`），再大取整、加千分位（`$123`、`$1,235`）。
    /// 不是美元的（cca 现在只给 USD）前面写货币代码：`EUR 12.35`。
    static func money(_ v: Double, currency: String = "USD") -> String {
        let sign = moneySign(currency)
        guard v.isFinite, v > 0 else { return sign + "0" }
        if v < 0.01 { return "<" + sign + "0.01" }
        let cents = (v * 100).rounded()
        if cents < 10000 { return sign + String(format: "%.2f", cents / 100) }
        return sign + grouped(Int(v.rounded()))
    }

    /// 纵轴的刻度：去掉末尾的 0（`$0`、`$2.5`、`$0.25`、`$1,250`）
    static func moneyAxis(_ v: Double, currency: String = "USD") -> String {
        let sign = moneySign(currency)
        guard v.isFinite, v > 0 else { return sign + "0" }
        if v >= 1000, v == v.rounded() { return sign + grouped(Int(v)) }
        var s = String(format: "%.2f", v)
        while s.hasSuffix("0") { s.removeLast() }
        if s.hasSuffix(".") { s.removeLast() }
        return sign + s
    }

    private static func moneySign(_ currency: String) -> String {
        currency.uppercased() == "USD" || currency.isEmpty ? "$" : currency + " "
    }

    /// 整数加千分位：`1,234,567`
    static func grouped(_ n: Int) -> String {
        let digits = String(n.magnitude)
        var out = ""
        for (i, ch) in digits.enumerated() {
            if i > 0, (digits.count - i) % 3 == 0 { out.append(",") }
            out.append(ch)
        }
        return (n < 0 ? "-" : "") + out
    }

    /// token 数：不到一千照写，再大写成 `1.2K`、`3.4M`、`1.2B`（100 以上不带小数：`210M`；四舍五入以后到一千的进一级：`1M`，不写 `1000K`）
    static func tokens(_ n: Int) -> String {
        guard n >= 1000 else { return "\(max(n, 0))" }
        var value = Double(n), unit = ""
        for u in ["K", "M", "B"] where value >= 999.5 {
            value /= 1000
            unit = u
        }
        var s = String(format: value >= 99.95 ? "%.0f" : "%.1f", value)
        if s.hasSuffix(".0") { s.removeLast(2) }
        return s + unit
    }

    /// 一项的 token：「输入 120K · 输出 802K · 缓存读 210M · 缓存写 4M」
    static func costTokens(_ t: CostTokens) -> String {
        "输入 \(tokens(t.input)) · 输出 \(tokens(t.output)) · 缓存读 \(tokens(t.cacheRead)) · 缓存写 \(tokens(t.cacheWrite))"
    }

    // MARK: 日期

    /// cca 给的本地日期（`2026-11-07`）：年、月、日；格式不对是 nil
    static func costDateParts(_ s: String) -> (year: Int, month: Int, day: Int)? {
        let parts = s.split(separator: "-", omittingEmptySubsequences: false)
        guard parts.count == 3, parts[0].count == 4, let y = Int(parts[0]), let m = Int(parts[1]), let d = Int(parts[2]),
              (1...12).contains(m), (1...31).contains(d) else { return nil }
        return (y, m, d)
    }

    /// 价格表的日期：「2026 年 11 月 1 日」（认不出就原样）
    static func costPricesDate(_ s: String) -> String {
        guard let p = costDateParts(s) else { return s }
        return "\(p.year) 年 \(p.month) 月 \(p.day) 日"
    }

    /// 一天在本地的哪个时刻（取正午，算星期几、写日期用）：有 `date` 用它（cca 按本地日期给的公历 YYYY-MM-DD），没有用这一项的正中间。
    /// 按公历解析（系统的日历设成佛历、和历时，calendar 里的“2026 年”是另一年），时区跟着 calendar
    private static func costDayEpoch(_ p: CostPeriod, calendar: Calendar) -> Double {
        var gregorian = Calendar(identifier: .gregorian)
        gregorian.timeZone = calendar.timeZone
        if let s = p.date, let d = costDateParts(s),
           let t = gregorian.date(from: DateComponents(year: d.year, month: d.month, day: d.day, hour: 12)) {
            return t.timeIntervalSince1970
        }
        return (p.startEpoch + p.endEpoch) / 2
    }

    /// 一项的起止（鼠标指着柱子时、读屏）：「11 月 4 日–11 日」「10 月 27 日–11 月 3 日」；在本地零点结束的（按日历切的周）写到前一天：
    /// 「11 月 3 日–9 日」。一天写「11 月 6 日 周五」。不是今年的带年份
    static func costRange(_ p: CostPeriod, granularity g: CostGranularity, now: Double, calendar: Calendar = .current) -> String {
        if g == .day {
            let t = costDayEpoch(p, calendar: calendar)
            let wd = calendar.component(.weekday, from: Date(timeIntervalSince1970: t))
            return dayText(t, now: now, calendar: calendar) + " 周" + weekdays[((wd - 1) % 7 + 7) % 7]
        }
        let end = Date(timeIntervalSince1970: p.endEpoch)
        let last = calendar.startOfDay(for: end) == end ? p.endEpoch - 1 : p.endEpoch
        let start = dayText(p.startEpoch, now: now, calendar: calendar)
        let a = calendar.dateComponents([.year, .month, .day], from: Date(timeIntervalSince1970: p.startEpoch))
        let b = calendar.dateComponents([.year, .month, .day], from: Date(timeIntervalSince1970: last))
        if a.year == b.year && a.month == b.month {
            return a.day == b.day ? start : start + "–\(b.day ?? 0) 日"
        }
        return start + "–" + dayText(last, now: now, calendar: calendar)
    }

    /// 柱子下面的日期：按周的是这一项从哪天开始（「11/4」），还没过完的写「本周」；按天的是那一天（「11/6」），今天写「今天」
    static func costLabel(_ p: CostPeriod, granularity g: CostGranularity, calendar: Calendar = .current) -> String {
        if p.current { return g == .week ? "本周" : "今天" }
        if g == .day, let s = p.date, let d = costDateParts(s) { return "\(d.month)/\(d.day)" }
        let t = g == .day ? costDayEpoch(p, calendar: calendar) : p.startEpoch
        let c = calendar.dateComponents([.month, .day], from: Date(timeIntervalSince1970: t))
        return "\(c.month ?? 0)/\(c.day ?? 0)"
    }

    // MARK: 句子

    /// 鼠标指着一根柱子时（和读屏）：「11 月 4 日–11 日：$1,021.40 · 2,210 次请求」，还没过完的「本周到现在（11 月 4 日–11 日）：…」
    /// 「今天到现在：…」；一台设备的没有请求数。这一项开始时还没有记录、有没有公开价格的模型时补一句
    static func costBarDetail(_ bar: CostBar, granularity g: CostGranularity, scope: CostScope, currency: String,
                              now: Double, calendar: Calendar = .current) -> String {
        let p = bar.period
        let head: String
        if p.current {
            head = g == .week ? "本周到现在（" + costRange(p, granularity: g, now: now, calendar: calendar) + "）" : "今天到现在"
        } else {
            head = costRange(p, granularity: g, now: now, calendar: calendar)
        }
        var s = head + "：" + money(bar.usd, currency: currency)
        if scope == .account, p.requests > 0 { s += " · " + grouped(p.requests) + " 次请求" }
        var notes: [String] = []
        if p.partial { notes.append("这一项开始时还没有记录，可能偏小") }
        if scope == .account, p.unpricedRequests > 0 { notes.append("其中 \(grouped(p.unpricedRequests)) 次请求的模型没有公开价格，没算钱") }
        return s + notes.map { "；" + $0 }.joined()
    }

    /// 图下面那一行（鼠标没指着柱子时）：按周「本周 $483.51 · 平均每周 $988.78（7 周）· 近 12 周合计 $8,052」，
    /// 按天「今天 $38.91 · 近 30 天合计 $4,200」。都是 cca 给的 summary，平均算不出来（一项都没过完）时不写
    static func costCaption(_ s: CostSummary, granularity g: CostGranularity, currency: String) -> String {
        switch g {
        case .week:
            var parts = ["本周 " + money(s.currentUSD, currency: currency)]
            if let avg = s.averageUSD {
                parts.append("平均每周 " + money(avg, currency: currency) + (s.averageWeeks > 0 ? "（\(s.averageWeeks) 周）" : ""))
            }
            parts.append("近 12 周合计 " + money(s.weeksUSD, currency: currency))
            return parts.joined(separator: " · ")
        case .day:
            return "今天 " + money(s.todayUSD, currency: currency) + " · 近 30 天合计 " + money(s.daysUSD, currency: currency)
        }
    }

    /// 一直显示的小字：按谁的价格折算、价格表是哪天的，不是实际花费
    static func costFootnote(_ tools: [ToolID], pricesDate: String?) -> String {
        var vendors: [String] = []
        for t in tools {
            let v: String
            switch t {
            case .claude: v = "Anthropic"
            case .codex: v = "OpenAI"
            case .agy: v = "Google"
            default: continue
            }
            if !vendors.contains(v) { vendors.append(v) }
        }
        let who = vendors.isEmpty ? "官方 API" : " " + vendors.joined(separator: "、") + " 官方 API"
        return "按\(who) 的价格折算" + (pricesDate.map { "（价格表 \(costPricesDate($0))）" } ?? "") + "，不是实际花费：订阅不按这个收费。"
    }

    /// 同步的状态、还没算进来的设备（只在要说的时候有）：
    /// old_server、not_logged_in、pending 说一句（灰字）；error、30 天内用过却没同步过金额的设备（missing）、很久没同步上来的设备说一句（橙色）。
    /// local_only、off、ok 不说。撤销了的设备不提醒（数字本来就是撤销前同步来的）。
    /// 看一台设备时（scope 是 .device）只说和它的数字有关的：别的设备同步来的才说同步的状态（这台设备自己的不经同步），
    /// 只说它自己很久没同步上来；别的设备没算进来、没同步上来不影响它的数字，不说
    static func costNotes(_ c: AccountCost, scope: CostScope = .account, now: Double, calendar: Calendar = .current) -> [CostNote] {
        let device = CostChart.resolve(scope, in: c).deviceID.flatMap { c.device($0) }
        var out: [CostNote] = []
        if device?.isMe != true {
            switch c.syncState {
            case .oldServer: out.append(CostNote(text: "只算了这台设备：维护者的设备统计服务器还不支持同步金额"))
            case .notLoggedIn: out.append(CostNote(text: "这台设备没登录这个账号，显示的是上次同步来的"))
            case .pending: out.append(CostNote(text: "还没和别的设备同步过"))
            case .error: out.append(CostNote(text: "上次同步没成功" + (c.sync?.message.map { "：" + $0 } ?? ""), warning: true))
            default: break
            }
        }
        let missing = c.sync?.missing ?? []
        if device == nil, !missing.isEmpty {
            out.append(CostNote(text: "还有 \(missing.count) 台设备没有算进来（\(names(missing.map(\.name)))，可能 cca 太旧）", warning: true))
        }
        for d in c.devices where device == nil || d.id == device?.id {
            if let text = costStale(d, now: now, calendar: calendar) { out.append(CostNote(text: text, warning: true)) }
        }
        return out
    }

    /// 一台很久没同步上来的设备（cca 标的 `stale`；这台设备自己、撤销了的不说）：「office-imac 周四 15:00 以后的还没同步上来」
    static func costStale(_ d: CostDevice, now: Double, calendar: Calendar = .current) -> String? {
        guard d.stale, !d.revoked, !d.isMe else { return nil }
        return d.syncedEpoch.map { spaced(d.name, pastTime($0, now: now, calendar: calendar)) + " 以后的还没同步上来" }
            ?? d.name + " 最近用的还没同步上来"
    }

    /// 「a、b、c」，再多写「a、b、c 等」（几台前面已经说了）
    private static func names(_ list: [String]) -> String {
        list.count <= 3 ? list.joined(separator: "、") : list.prefix(3).joined(separator: "、") + " 等"
    }

    private static func spaced(_ a: String, _ b: String) -> String { a + " " + b }

    /// 还没有记录（这个账号在这台设备上没用过，别的设备也没同步来）
    static func costEmpty(_ tool: ToolID) -> String {
        "还没有记录：用 \(cliName(tool)) 干过活以后会显示在这里。"
    }

    /// 还没统计过，现在也没在统计（上次没统计成，或者用户走开了）：App 过一会儿自己再试
    static let costNotScanned = "还没统计：过一会儿会自动统计，也可以点「刷新」。"

    /// 从什么时候开始有记录，哪些会话算不进来（显示细节时）
    static func costSince(_ epoch: Double, tool: ToolID, now: Double, calendar: Calendar = .current) -> String {
        let since = "从 " + dayText(epoch, now: now, calendar: calendar) + "开始有记录。"
        switch tool {
        case .codex: return since + "Codex 0.157 起的会话才带着账号信息，更早的算不进来。"
        default: return since + "Claude Code 2.1.317 起的会话才带着账号信息，更早的、Claude 桌面 App 里的会话算不进来。"
        }
    }

    /// 没有公开价格的模型（显示细节时）：「这些模型没有公开价格，只计了请求数：x、y（近 12 周 4 次请求）」
    static func costUnpriced(_ models: [String], requests: Int) -> String? {
        guard !models.isEmpty else { return nil }
        return "这些模型没有公开价格，只计了请求数：" + models.joined(separator: "、") + (requests > 0 ? "（近 12 周 \(grouped(requests)) 次请求）" : "")
    }

    /// 上次和别的设备同步成功（显示细节时）
    static func costSynced(_ epoch: Double, now: Double, calendar: Calendar = .current) -> String {
        "上次和别的设备同步：" + pastTime(epoch, now: now, calendar: calendar)
    }

    /// 设备的叫法（选看哪台设备的菜单、用量统计页各台设备那一行）：这台设备「studio-mac（这台）」，撤销了的「old-mini（已撤销）」
    static func costDeviceName(_ d: CostDevice) -> String {
        d.name + (d.isMe ? "（这台）" : d.revoked ? "（已撤销）" : "")
    }

    /// 多久以前统计的（标题右边）：cca 正在做第一次统计时「正在统计…」，还没统计过、也没在统计时「还没统计」
    static func costFreshness(_ scan: CostScan?, now: Double) -> String? {
        guard let scan else { return nil }
        if scan.state == .scanning { return "正在统计…" }
        if scan.state == .never { return "还没统计" }
        return scan.updatedEpoch.map { agoPhrase(now - $0) + "统计" }
    }

    /// 认不出是哪个账号的用量（用量统计页的合计下面）：没有时是 nil
    static func costUnattributed(_ u: CostUnattributed?, currency: String) -> String? {
        guard let u else { return nil }
        let what: String
        if u.daysUSD > 0 {
            what = money(u.daysUSD, currency: currency) + " "
        } else if u.requests > 0 {
            what = "\(grouped(u.requests)) 次请求"
        } else {
            return nil
        }
        return "近 30 天另有 \(what)认不出是哪个账号的（旧版 Claude Code、桌面 App 里的会话），没算进来。"
    }
}
