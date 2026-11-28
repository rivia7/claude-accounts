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
            head = g == .week ? "本周至今（" + costRange(p, granularity: g, now: now, calendar: calendar) + "）" : "今天"
        } else {
            head = costRange(p, granularity: g, now: now, calendar: calendar)
        }
        var s = head + "：" + money(bar.usd, currency: currency)
        if scope == .all, p.requests > 0 { s += " · " + grouped(p.requests) + " 次请求" }
        var notes: [String] = []
        if p.partial { notes.append("这一项开始时还没有记录，可能偏小") }
        if scope == .all, p.unpricedRequests > 0 { notes.append("其中 \(grouped(p.unpricedRequests)) 次请求的模型没有公开价格，未计入金额") }
        return s + notes.map { "；" + $0 }.joined()
    }

    /// 图下面那一行（鼠标没指着柱子时）：按周「本周 $483.51 · 平均每周 $988.78（7 周）· 近 12 周合计 $8,052 · 缓存命中 97.3%」，
    /// 按天「今天 $38.91 · 平均每天 $140.20（29 天）· 近 30 天合计 $4,200 · 缓存命中 97.5%」。都是 cca 给的 summary，平均算不出来（一项都没过完）时不写，
    /// 缓存命中率 cca 没给（没有输入、旧版 cca）时不写
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
        let hit = costCacheHit(s.cacheHit(g)).map { "缓存命中 " + $0 }
        return ([current] + (avg.map { [$0] } ?? []) + [total] + (hit.map { [$0] } ?? [])).joined(separator: " · ")
    }

    /// 缓存命中率（cca 给的 0～1）：「97.3%」，千分之一往下取（没到 100% 的不写成 100%）；cca 没给（没有输入、旧版 cca）时是 nil
    static func costCacheHit(_ v: Double?) -> String? {
        guard let v, v.isFinite else { return nil }
        let tenths = Int((min(max(v, 0), 1) * 1000 + 1e-6).rounded(.down))
        return (tenths % 10 == 0 ? "\(tenths / 10)" : "\(tenths / 10).\(tenths % 10)") + "%"
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
        return "按\(who) 的价格折算" + (pricesDate.map { "（价格表 \(costPricesDate($0))）" } ?? "") + "，不是实际花费：订阅并不按此收费。"
    }

    /// 同步的状态、还没算进来的设备（看所有设备时，只在要说的时候有）：
    /// not_logged_in、pending、local_only、off 说一句（灰字）；error（和不认识的状态）、30 天内用过却没同步过金额的设备（missing）、
    /// 很久没同步上来的设备说一句（橙色）。ok 不说。撤销了的设备不提醒（数字本来就是撤销前同步来的）。cca 给的诊断（diag）不说。
    /// 只看一台设备时只说那一台的：本机什么都不说（这台设备自己的数字不经同步，别的设备没算进来、没同步上来不影响它），
    /// 另一台设备说它很久没同步上来
    static func costNotes(_ c: AccountCost, scope: CostScope = .all, now: Double, calendar: Calendar = .current) -> [CostNote] {
        guard scope == .all else {
            guard let d = CostChart.device(c, scope: scope) else { return [] }
            return [costStale(d, now: now, calendar: calendar)].compactMap { $0 }.map { CostNote(text: $0, warning: true) }
        }
        var out: [CostNote] = []
        // cca 没给 sync 时（syncState 当作 local_only）不知道为什么只有这台设备，不说
        switch c.sync?.state.shown {
        case .notLoggedIn?: out.append(CostNote(text: "这台设备未登录这个账号，显示的是上次同步的数据"))
        case .pending?: out.append(CostNote(text: "尚未与其他设备同步"))
        case .localOnly?: out.append(CostNote(text: "只统计了这台设备：这个账号不是 Team 分配的，或这台设备无法使用那个 Team 的服务器"))
        case .off?: out.append(CostNote(text: "只统计了这台设备：这台设备关闭了设备统计"))
        case .error?: out.append(CostNote(text: "上次同步失败" + (c.sync?.message.map { "：" + $0 } ?? ""), warning: true))
        default: break
        }
        let missing = c.sync?.missing ?? []
        if !missing.isEmpty {
            out.append(CostNote(text: "还有 \(missing.count) 台设备未统计在内（\(names(missing.map(\.name)))，可能 cca 版本太旧）", warning: true))
        }
        for d in c.devices {
            if let text = costStale(d, now: now, calendar: calendar) { out.append(CostNote(text: text, warning: true)) }
        }
        return out
    }

    /// 一台很久没同步上来的设备（cca 标的 `stale`；这台设备自己、撤销了的不说）：「office-imac 周四 15:00 以后的还没同步上来」
    static func costStale(_ d: CostDevice, now: Double, calendar: Calendar = .current) -> String? {
        guard d.stale, !d.revoked, !d.isMe else { return nil }
        return d.syncedEpoch.map { spaced(d.name, pastTime($0, now: now, calendar: calendar)) + " 之后的数据还没同步过来" }
            ?? d.name + " 最近的用量还没同步过来"
    }

    /// 「a、b、c」，再多写「a、b、c 等」（几台前面已经说了）
    private static func names(_ list: [String]) -> String {
        list.count <= 3 ? list.joined(separator: "、") : list.prefix(3).joined(separator: "、") + " 等"
    }

    private static func spaced(_ a: String, _ b: String) -> String { a + " " + b }

    /// 还没有记录（这个账号在这台设备上没用过，别的设备也没同步来）
    static func costEmpty(_ tool: ToolID) -> String {
        "还没有记录：用 \(cliName(tool)) 工作后会显示在这里。"
    }

    /// 还没统计过，现在也没在统计（上次没统计成，或者用户走开了）：App 过一会儿自己再试
    static let costNotScanned = "尚未统计：稍后会自动统计，也可以点「刷新」。"

    /// 从什么时候开始有记录，哪些会话算不进来（显示细节时）
    static func costSince(_ epoch: Double, tool: ToolID, now: Double, calendar: Calendar = .current) -> String {
        let since = "从 " + dayText(epoch, now: now, calendar: calendar) + "开始有记录。"
        switch tool {
        case .codex: return since + "Codex 0.157 及以后的会话才带有账号信息，更早的无法统计。"
        case .agy: return since + "只有用 cca 启动的 Antigravity 会话能识别账号，直接运行 agy 的会计入「无法识别账号」。"
        default: return since
        }
    }

    /// 没有公开价格的模型（显示细节时）：「这些模型没有公开价格，只计了请求数：x、y（近 12 周 4 次请求）」
    static func costUnpriced(_ models: [String], requests: Int) -> String? {
        guard !models.isEmpty else { return nil }
        return "这些模型没有公开价格，只统计了请求数：" + models.joined(separator: "、") + (requests > 0 ? "（近 12 周 \(grouped(requests)) 次请求）" : "")
    }

    /// 上次和别的设备同步成功（显示细节时）
    static func costSynced(_ epoch: Double, now: Double, calendar: Calendar = .current) -> String {
        "上次与其他设备同步：" + pastTime(epoch, now: now, calendar: calendar)
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
        return "还没有 Claude、ChatGPT、Antigravity 账号。第三方 API 账号按实际用量付费，不在这里折算。"
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
        if scan.state == .never { return "尚未统计" }
        return scan.updatedEpoch.map { agoPhrase(now - $0) + "统计" }
    }

    /// 认不出是哪个账号的用量（折合金额页的合计下面）。cca 给了所有设备的（`unattributed.cost`，3.6.8 起）：说没算进合计、单独列在
    /// 最下面（本机、另一台设备时说那一台的；它没有这种用量时不说）。旧版 cca 只有这台设备近 30 天的：照旧说这台设备的金额，
    /// 看另一台设备时不说。不说为什么认不出（cca 给的原因不显示）。没有时是 nil
    static func costUnattributed(_ u: CostUnattributed?, scope: CostScope = .me, total: CostTotal? = nil, currency: String) -> String? {
        guard let u else { return nil }
        if let c = u.cost {
            guard scope == .all || CostChart.device(c, scope: scope) != nil else { return nil }
            let who: String
            switch scope {
            case .all: who = ""
            case .me: who = "这台设备"
            case .device: who = costOrphanRowName(scope, total: total) + " "
            }
            return who + "无法识别账号的用量不计入合计，单独列在最下面。"
        }
        guard !scope.isOtherDevice else { return nil }
        let what: String
        if u.daysUSD > 0 {
            what = money(u.daysUSD, currency: currency) + " "
        } else if u.requests > 0 {
            what = "\(grouped(u.requests)) 次请求"
        } else {
            return nil
        }
        return "近 30 天另有 \(what)无法识别属于哪个账号，未计入。"
    }

    /// 还没传认不出的用量的设备（cca 给的 `unattributed.unsynced`，3.6.8 起）：看所有设备时
    /// 「还有 2 台设备没传认不出账号的用量（a、b），可能 cca 太旧。」，看其中一台时「office-imac 还没传认不出账号的用量，可能 cca 太旧。」；
    /// 本机、都传了时是 nil
    static func costUnsynced(_ u: CostUnattributed?, scope: CostScope) -> String? {
        guard let list = u?.unsynced, !list.isEmpty else { return nil }
        switch scope {
        case .all: return "还有 \(list.count) 台设备没有上传无法识别账号的用量（\(names(list.map(\.name)))），可能 cca 版本太旧。"
        case .me: return nil
        case .device(let id): return list.first { $0.id == id }.map { $0.name + " 还没有上传无法识别账号的用量，可能 cca 版本太旧。" }
        }
    }

    /// 「认不出账号」那一行的名字：看所有设备时「所有设备」，本机「这台设备」，另一台设备是它的名字
    static func costOrphanRowName(_ scope: CostScope, total: CostTotal?) -> String {
        switch scope {
        case .all: return "所有设备"
        case .me: return "这台设备"
        case .device(let id): return total?.device(id)?.name ?? total?.unattributed?.cost?.device(id)?.name ?? "那台设备"
        }
    }

    // MARK: 按模型

    /// 按模型那一块的小标题：「按模型 · 近 12 周」「按模型 · 近 30 天」
    static func costModelsTitle(_ g: CostGranularity) -> String {
        "按模型 · " + (g == .week ? "近 12 周" : "近 30 天")
    }

    /// 一个模型的叫法：模型名；会话记录里没写的「没写模型名」；没有按模型记下的那一部分「没按模型记下的」
    static func costModelName(_ r: CostModelRow) -> String {
        guard let m = r.model else { return "未按模型记录的" }
        return m.isEmpty ? "未记录模型名" : m
    }

    /// 金额那一栏：都没有公开价格的写「没有公开价格」；按同系列的标价估算的新模型（cca 给的 priced_as）前面加「≈」
    static func costModelMoney(_ x: CostModelRange, currency: String, estimated: Bool = false) -> String {
        x.allUnpriced ? "没有公开价格" : (estimated ? "≈ " : "") + money(x.usd, currency: currency)
    }

    /// 请求数那一栏：「8,804 次」
    static func costModelRequests(_ x: CostModelRange) -> String {
        grouped(x.requests) + " 次"
    }

    /// 缓存命中率那一栏：「命中 97.3%」；cca 没给（这一段没有输入、旧版 cca）时空着
    static func costModelHit(_ x: CostModelRange) -> String {
        costCacheHit(x.cacheHit).map { "命中 " + $0 } ?? ""
    }

    /// 读屏：「$790，8,804 次请求，缓存命中 97.3%」「没有公开价格，316 次请求」；有一部分没算钱时补一句
    static func costModelValue(_ r: CostModelRow, granularity g: CostGranularity, currency: String) -> String {
        let x = r.range(g)
        var s = costModelMoney(x, currency: currency, estimated: r.pricedAs != nil) + "，" + grouped(x.requests) + " 次请求"
        if let hit = costCacheHit(x.cacheHit) { s += "，缓存命中 " + hit }
        if !x.allUnpriced, x.unpricedRequests > 0 { s += "，其中 \(grouped(x.unpricedRequests)) 次没有公开价格、未计入金额" }
        if !x.allUnpriced, let base = r.pricedAs { s += "，按 \(base) 的价格估算" }
        return s
    }

    /// 悬停提示：读屏那一句；没按模型记下的那一行再说为什么
    static func costModelHelp(_ r: CostModelRow, granularity g: CostGranularity, currency: String) -> String {
        let value = costModelValue(r, granularity: g, currency: currency)
        if r.model == nil { return value + "\n旧版本统计的、或旧版 cca 设备上传的数据只有合计，没有按模型区分" }
        if r.pricedAs != nil { return value + "\ncca 的价格表中还没有这个新模型：暂按同系列的价格估算，收录价格后会自动重算" }
        return value
    }

    /// 按模型那一块下面：有按同系列的标价估算的新模型（cca 给的 priced_as）时说一句（这一段里没有用量、都没算钱的不算）
    static func costEstimated(_ rows: [CostModelRow], granularity g: CostGranularity) -> String? {
        let est = rows.filter { $0.pricedAs != nil && $0.model != nil && !$0.range(g).isEmpty && !$0.range(g).allUnpriced }
        guard let first = est.first, let model = first.model, let base = first.pricedAs else { return nil }
        if est.count == 1 { return "检测到新模型 \(model)：cca 的价格表中还没有它，暂按 \(base) 的价格估算（≈）" }
        return "检测到新模型 " + est.compactMap(\.model).joined(separator: "、") + "：cca 的价格表中还没有，暂按同系列的价格估算（≈）"
    }

}
