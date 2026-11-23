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
    /// 「今天到现在：…」；只看一台设备（本机、另一台）时没有请求数（cca 只给整个账号的）。这一项开始时还没有记录、有没有公开价格的模型时补一句
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
        if scope == .all, p.requests > 0 { s += " · " + grouped(p.requests) + " 次请求" }
        var notes: [String] = []
        if p.partial { notes.append("这一项开始时还没有记录，可能偏小") }
        if scope == .all, p.unpricedRequests > 0 { notes.append("其中 \(grouped(p.unpricedRequests)) 次请求的模型没有公开价格，没算钱") }
        return s + notes.map { "；" + $0 }.joined()
    }

    /// 图下面那一行（鼠标没指着柱子时）：按周「本周 $483.51 · 平均每周 $988.78（7 周）· 近 12 周合计 $8,052」，
    /// 按天「今天 $38.91 · 平均每天 $140.20（29 天）· 近 30 天合计 $4,200」。都是 cca 给的 summary，平均算不出来（一项都没过完）时不写
    static func costCaption(_ s: CostSummary, granularity g: CostGranularity, currency: String) -> String {
        let (current, average, count, total): (String, Double?, String?, String)
        switch g {
        case .week:
            (current, average, total) = ("本周 " + money(s.currentUSD, currency: currency), s.averageUSD,
                                         "近 12 周合计 " + money(s.weeksUSD, currency: currency))
            count = s.averageWeeks > 0 ? "（\(s.averageWeeks) 周）" : nil
        case .day:
            (current, average, total) = ("今天 " + money(s.todayUSD, currency: currency), s.averageDayUSD,
                                         "近 30 天合计 " + money(s.daysUSD, currency: currency))
            count = s.averageDays > 0 ? "（\(s.averageDays) 天）" : nil
        }
        let avg = average.map { (g == .week ? "平均每周 " : "平均每天 ") + money($0, currency: currency) + (count ?? "") }
        return ([current] + (avg.map { [$0] } ?? []) + [total]).joined(separator: " · ")
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

    /// 同步的状态、还没算进来的设备（看所有设备时，只在要说的时候有）：
    /// not_logged_in、pending、local_only、off 说一句（灰字）；error（和不认识的状态）、30 天内用过却没同步过金额的设备（missing）、
    /// 很久没同步上来的设备、有用量没算进来的设备（cca 给的 diag.why）说一句（橙色）。ok 不说。撤销了的设备不提醒（数字本来就是撤销前同步来的）。
    /// 只看一台设备时只说那一台的：本机只说有用量没算进来（这台设备自己的数字不经同步，别的设备没算进来、没同步上来不影响它），
    /// 另一台设备再说它很久没同步上来
    static func costNotes(_ c: AccountCost, scope: CostScope = .all, now: Double, calendar: Calendar = .current) -> [CostNote] {
        guard scope == .all else {
            guard let d = CostChart.device(c, scope: scope) else { return [] }
            return [costStale(d, now: now, calendar: calendar), costDeviceNote(d)].compactMap { $0 }.map { CostNote(text: $0, warning: true) }
        }
        var out: [CostNote] = []
        // cca 没给 sync 时（syncState 当作 local_only）不知道为什么只有这台设备，不说
        switch c.sync?.state.shown {
        case .notLoggedIn?: out.append(CostNote(text: "这台设备没登录这个账号，显示的是上次同步来的"))
        case .pending?: out.append(CostNote(text: "还没和别的设备同步过"))
        case .localOnly?: out.append(CostNote(text: "只算了这台设备：这个账号不是哪个 Team 给的，或者这台设备用不了那个 Team 的服务器"))
        case .off?: out.append(CostNote(text: "只算了这台设备：这台设备关掉了设备统计"))
        case .error?: out.append(CostNote(text: "上次同步没成功" + (c.sync?.message.map { "：" + $0 } ?? ""), warning: true))
        default: break
        }
        let missing = c.sync?.missing ?? []
        if !missing.isEmpty {
            out.append(CostNote(text: "还有 \(missing.count) 台设备没有算进来（\(names(missing.map(\.name)))，可能 cca 太旧）", warning: true))
        }
        for d in c.devices {
            if let text = costStale(d, now: now, calendar: calendar) { out.append(CostNote(text: text, warning: true)) }
        }
        for d in c.devices {
            if let text = costDeviceNote(d) { out.append(CostNote(text: text, warning: true)) }
        }
        return out
    }

    /// 一个认不出账号的原因（几个用「、」连起来）：「Claude Code 2.1.316 太旧」「Claude App 里的 Code 会话」「会话没带账号信息」
    static func costWhy(_ w: CostWhy) -> String {
        switch w.code.shown {
        case .oldClaudeCode: return "Claude Code " + (w.claudeCode.map { $0 + " " } ?? "") + "太旧"
        case .claudeApp: return "Claude App 里的 Code 会话"
        default: return "会话没带账号信息"
        }
    }

    /// 一台设备有用量没算进来（cca 给的 diag.why；撤销了的不说）：「有用量没算进来：Claude Code 2.1.316 太旧」（设备那一行的提示）。
    /// 都认得出、旧版 cca 的设备（没有诊断）是 nil
    static func costDeviceWhy(_ d: CostDevice) -> String? {
        guard !d.revoked, let why = d.diag?.why, !why.isEmpty else { return nil }
        return "有用量没算进来：" + why.map(costWhy).joined(separator: "、")
    }

    /// 同上，带着是哪台设备，命令行里用的 Claude Code 还是旧版本（cca 标的 claude_code_old）时提醒更新：
    /// 「office-imac 有用量没算进来：Claude Code 2.1.316 太旧，那台设备要更新 Claude Code」「这台设备有用量没算进来：…」
    static func costDeviceNote(_ d: CostDevice) -> String? {
        guard let why = costDeviceWhy(d) else { return nil }
        let update = d.diag?.claudeCodeOld == true ? (d.isMe ? "，要更新 Claude Code" : "，那台设备要更新 Claude Code") : ""
        return (d.isMe ? "这台设备" : d.name + " ") + why + update
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

    /// 设备的叫法（折合金额页各台设备那一行）：这台设备「studio-mac（这台）」，撤销了的「old-mini（已撤销）」
    static func costDeviceName(_ d: CostDevice) -> String {
        d.name + (d.isMe ? "（这台）" : d.revoked ? "（已撤销）" : "")
    }

    /// 设备菜单里的一台（cost_total.devices 里别的设备）：撤销了的「old-mini（已撤销）」，很久没同步上来的「office-imac（很久没同步）」
    /// （折合金额页那一行是名字后面一个橙色的钟，菜单里只能写字）
    static func costMenuName(_ d: CostTotalDevice) -> String {
        d.name + (d.revoked ? "（已撤销）" : d.stale ? "（很久没同步）" : "")
    }

    /// 账号页「折合金额」一根柱子也没有时：「最近 12 周没有用量。」「这台设备最近 30 天没有用量。」「bob-mini 最近 12 周没有用量。」
    /// （设备名和后面的中文之间空一格；cca 没列那台设备时不说是谁）
    static func costNoUsage(_ scope: CostScope, total: CostTotal?, granularity g: CostGranularity) -> String {
        let what = g == .week ? "最近 12 周没有用量。" : "最近 30 天没有用量。"
        switch scope {
        case .all: return what
        case .me: return "这台设备" + what
        case .device(let id): return total?.device(id).map { spaced($0.name, what) } ?? what
        }
    }

    /// 折合金额页一个账号都不列时：看另一台设备时说它最近没用过这里的账号（启用的工具里没有它用过的），别的时候是还没有官方账号
    static func costNoAccounts(_ scope: CostScope, total: CostTotal?) -> String {
        if case .device(let id) = scope {
            return (total?.device(id).map { $0.name + " " } ?? "那台设备") + "最近没有用过这里的账号。"
        }
        return "还没有 Claude、ChatGPT 账号。第三方 API 账号按实际用量付费，不在这里折算。"
    }

    /// 折合金额页合计那一块的读屏标签
    static func costTotalLabel(_ scope: CostScope, total: CostTotal?) -> String {
        switch scope {
        case .all: return "所有账号的折合金额"
        case .me: return "所有账号在这台设备上的折合金额"
        case .device(let id): return "所有账号在 " + (total?.device(id)?.name ?? "另一台设备") + " 上的折合金额"
        }
    }

    /// 多久以前统计的（标题右边）：cca 正在做第一次统计时「正在统计…」，还没统计过、也没在统计时「还没统计」
    static func costFreshness(_ scan: CostScan?, now: Double) -> String? {
        guard let scan else { return nil }
        if scan.state == .scanning { return "正在统计…" }
        if scan.state == .never { return "还没统计" }
        return scan.updatedEpoch.map { agoPhrase(now - $0) + "统计" }
    }

    /// 认不出是哪个账号的用量（折合金额页的合计下面，这台设备的）：为什么（cca 给的原因；旧版 cca 没给时笼统地说）。没有时是 nil
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
        let why = u.why.isEmpty ? "旧版 Claude Code、桌面 App 里的会话" : u.why.map(costWhy).joined(separator: "、")
        return "近 30 天另有 \(what)认不出是哪个账号的（\(why)），没算进来。"
    }

    /// 这台设备命令行里用的 Claude Code 太旧（cca 标的 claude_code_old，和有没有认不出的用量无关）：提醒更新，旁边是去「客户端更新」的按钮
    static func costUpdateClaudeCode(_ u: CostUnattributed?) -> String? {
        guard let u, u.claudeCodeOld else { return nil }
        return "这台设备命令行里用的 Claude Code " + (u.claudeCode.map { $0 + " " } ?? "")
            + "太旧，会话不带账号信息：不经 cca 启动的用量算不进来。更新 Claude Code 就好了。"
    }
}
