import CcaRemoteKit
import Foundation

// 折合金额、用量统计、总览、共享与同步的文字。说法照 Mac App（macos/Sources/CcaKit/Model/CostTexts.swift、Texts.swift 的同名函数）：
// 照实说、不用比喻；数字都是那台电脑上的 cca 算好的（CcaRemoteKit 的 AccountCost、HostCost、HostTeam），这里只排版

/// 一行提示（同步的状态、哪台设备还没算进来）：warning 的用橙色，别的灰字
struct CostNote: Equatable {
    let text: String
    var warning = false
}

extension Texts {
    private static let weekdays = ["日", "一", "二", "三", "四", "五", "六"]

    // MARK: 金额

    /// 金额（美分）：0 写 `$0`，不到 100 美元带两位小数（`$12.35`），再大取整、加千分位（`$123`、`$1,235`）
    static func money(cents: Int) -> String {
        guard cents > 0 else { return "$0" }
        if cents < 10000 { return String(format: "$%.2f", Double(cents) / 100) }
        return "$" + grouped(Int((Double(cents) / 100).rounded()))
    }

    /// 纵轴的刻度（美元）：去掉末尾的 0（`$0`、`$2.5`、`$0.25`、`$1,250`）
    static func moneyAxis(_ v: Double) -> String {
        guard v.isFinite, v > 0 else { return "$0" }
        if v >= 1000, v == v.rounded() { return "$" + grouped(Int(v)) }
        var s = String(format: "%.2f", v)
        while s.hasSuffix("0") { s.removeLast() }
        if s.hasSuffix(".") { s.removeLast() }
        return "$" + s
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

    /// 缓存命中率（cca 给的 0～1）：「97.3%」，千分之一往下取（没到 100% 的不写成 100%）；没有是 nil
    static func costCacheHit(_ v: Double?) -> String? {
        guard let v, v.isFinite else { return nil }
        let tenths = Int((min(max(v, 0), 1) * 1000 + 1e-6).rounded(.down))
        return (tenths % 10 == 0 ? "\(tenths / 10)" : "\(tenths / 10).\(tenths % 10)") + "%"
    }

    // MARK: 日期

    /// 价格表的日期：「2026 年 11 月 1 日」（认不出就原样）
    static func costPricesDate(_ s: String) -> String {
        let parts = s.split(separator: "-", omittingEmptySubsequences: false)
        guard parts.count == 3, parts[0].count == 4, let y = Int(parts[0]), let m = Int(parts[1]), let d = Int(parts[2]),
              (1...12).contains(m), (1...31).contains(d) else { return s }
        return "\(y) 年 \(m) 月 \(d) 日"
    }

    /// 一天取它的正中间算日期（按天的一项是那一天的 0 点到 24 点；那台电脑和这台手机的时区不一样时差几个小时也落在同一天）
    private static func dayMid(_ p: CostPeriod) -> Double { (p.startEpoch + p.endEpoch) / 2 }

    /// 一项的起止（点了柱子时、读屏）：「11 月 4 日–11 日」「10 月 27 日–11 月 3 日」；在本地零点结束的（按日历切的周）写到前一天。
    /// 一天写「11 月 6 日 周五」。不是今年的带年份
    static func costRange(_ p: CostPeriod, days: Bool, now: Double) -> String {
        let cal = Calendar.current
        if days {
            let t = dayMid(p)
            let wd = cal.component(.weekday, from: Date(timeIntervalSince1970: t))
            return dayText(t, now: now) + " 周" + weekdays[((wd - 1) % 7 + 7) % 7]
        }
        let end = Date(timeIntervalSince1970: p.endEpoch)
        let last = cal.startOfDay(for: end) == end ? p.endEpoch - 1 : p.endEpoch
        let start = dayText(p.startEpoch, now: now)
        let a = cal.dateComponents([.year, .month, .day], from: Date(timeIntervalSince1970: p.startEpoch))
        let b = cal.dateComponents([.year, .month, .day], from: Date(timeIntervalSince1970: last))
        if a.year == b.year && a.month == b.month {
            return a.day == b.day ? start : start + "–\(b.day ?? 0) 日"
        }
        return start + "–" + dayText(last, now: now)
    }

    /// 柱子下面的日期：按周的是这一项从哪天开始（「11/4」），还没过完的写「本周」；按天的是那一天（「11/6」），今天写「今天」
    static func costLabel(_ p: CostPeriod, days: Bool) -> String {
        if p.current { return days ? "今天" : "本周" }
        let c = Calendar.current.dateComponents([.month, .day], from: Date(timeIntervalSince1970: days ? dayMid(p) : p.startEpoch))
        return "\(c.month ?? 0)/\(c.day ?? 0)"
    }

    /// 过去的一个时刻：今天的写钟点，昨天的「昨天 15:00」，一周以内「周四 15:00」，再早写日期
    static func pastTime(_ epoch: Double, now: Double) -> String {
        let cal = Calendar.current
        let d = Date(timeIntervalSince1970: epoch), n = Date(timeIntervalSince1970: now)
        let days = cal.dateComponents([.day], from: cal.startOfDay(for: d), to: cal.startOfDay(for: n)).day ?? 0
        if days <= 0 { return UsageText.clock(epoch) }
        if days == 1 { return "昨天 " + UsageText.clock(epoch) }
        if days < 7 { return UsageText.weekdayTime(epoch) }
        return dayText(epoch, now: now)
    }

    // MARK: 折合金额的句子

    /// 点了一根柱子时（和读屏）：「11 月 4 日–11 日：$1,021.40」，还没过完的「本周至今（11 月 4 日–11 日）：…」「今天：…」；
    /// 这一项开始时还没有记录时补一句
    static func costBarDetail(_ p: CostPeriod, days: Bool, now: Double) -> String {
        let head = p.current ? (days ? "今天" : "本周至今（" + costRange(p, days: false, now: now) + "）") : costRange(p, days: days, now: now)
        return head + "：" + money(cents: p.cents) + (p.partial ? "；这一项开始时还没有记录，可能偏小" : "")
    }

    /// 图下面那一行：按周「本周 $483.51 · 平均每周 $988.78（7 周）· 近 12 周合计 $8,052 · 缓存命中 97.3%」，
    /// 按天「今天 $38.91 · 平均每天 $140.20（29 天）· 近 30 天合计 $4,200 · 缓存命中 97.5%」（平均算不出来、没有缓存命中率时不写）
    static func costCaption(_ c: AccountCost, days: Bool) -> String {
        let current = (days ? "今天 " : "本周 ") + money(cents: c.current(days: days))
        let n = c.averageCount(days: days)
        let avg = c.average(days: days).map {
            (days ? "平均每天 " : "平均每周 ") + money(cents: $0) + (n > 0 ? (days ? "（\(n) 天）" : "（\(n) 周）") : "")
        }
        let total = (days ? "近 30 天合计 " : "近 12 周合计 ") + money(cents: c.total(days: days))
        let hit = costCacheHit(c.cacheHit(days: days)).map { "缓存命中 " + $0 }
        return ([current, avg, total, hit] as [String?]).compactMap { $0 }.joined(separator: " · ")
    }

    /// 一直显示的小字：按谁的价格折算、价格表是哪天的，不是实际花费
    static func costFootnote(_ tools: [String], pricesDate: String?) -> String {
        var vendors: [String] = []
        for t in tools {
            let v: String
            switch t {
            case "claude": v = "Anthropic"
            case "codex": v = "OpenAI"
            case "agy": v = "Google"
            default: continue
            }
            if !vendors.contains(v) { vendors.append(v) }
        }
        let who = vendors.isEmpty ? "官方 API" : " " + vendors.joined(separator: "、") + " 官方 API"
        let date = pricesDate.flatMap(nonEmpty).map { "（价格表 \(costPricesDate($0))）" } ?? ""
        return "按\(who) 的价格折算" + date + "，不是实际花费：订阅并不按此收费。"
    }

    /// 同步的状态、还没算进来的设备、很久没同步上来的设备（Mac App 的 costNotes 看所有设备时的那几句；cca 给的出错说明手机上没有）
    static func costNotes(_ c: AccountCost, now: Double) -> [CostNote] {
        var out: [CostNote] = []
        switch c.sync {
        case "not_logged_in": out.append(CostNote(text: "电脑未登录这个账号，显示的是上次同步的数据"))
        case "pending": out.append(CostNote(text: "尚未与其他设备同步"))
        case "local_only": out.append(CostNote(text: "只统计了一台电脑：这个账号不是 Team 分配的，或电脑无法使用那个 Team 的服务器"))
        case "off": out.append(CostNote(text: "只统计了一台电脑：电脑关闭了设备统计"))
        case "", "ok": break
        default: out.append(CostNote(text: "上次同步失败", warning: true))
        }
        if c.missing > 0 {
            out.append(CostNote(text: "还有 \(c.missing) 台设备未统计在内（可能 cca 版本太旧）", warning: true))
        }
        for d in c.devices {
            if let text = costStale(d, now: now) { out.append(CostNote(text: text, warning: true)) }
        }
        return out
    }

    /// 一台很久没同步上来的设备（放上来的那台电脑自己不说）：「office-imac 周四 15:00 之后的数据还没同步过来」
    static func costStale(_ d: CostDevice, now: Double) -> String? {
        guard d.stale, !d.me else { return nil }
        return d.syncedEpoch.map { d.name + " " + pastTime($0, now: now) + " 之后的数据还没同步过来" } ?? d.name + " 最近的用量还没同步过来"
    }

    /// 设备的叫法：放上来的那台「studio-mac（这台）」
    static func costDeviceName(_ d: CostDevice) -> String {
        d.name + (d.me ? "（这台）" : "")
    }

    /// 多久以前统计的（账号页「折合金额」最下面）：「电脑上 4 分钟前统计。」；正在做第一次统计、还没统计过时照实说
    static func costFreshness(_ h: HostCost?, now: Double) -> String? {
        guard let h else { return nil }
        if h.scanning { return "电脑正在统计，首次需要读取全部会话记录。" }
        return h.updatedEpoch.map { "电脑上" + agoPhrase(now - $0) + "统计。" } ?? "电脑上还没有统计过。"
    }

    /// 还没有记录（这个账号在那台电脑上没用过，别的设备也没同步来）
    static func costEmpty(_ tool: String) -> String {
        "还没有记录：用 \(tool == "codex" ? "Codex" : tool == "agy" ? "agy" : "Claude Code") 工作后会显示在这里。"
    }

    /// 按模型那一块的小标题：「按模型 · 近 12 周」「按模型 · 近 30 天」
    static func costModelsTitle(days: Bool) -> String {
        "按模型 · " + (days ? "近 30 天" : "近 12 周")
    }

    /// 一个模型的叫法：模型名；会话记录里没写的「未记录模型名」；没有按模型记下的那一部分「未按模型记录的」
    static func costModelName(_ m: CostModel) -> String {
        guard let name = m.name else { return "未按模型记录的" }
        return name.isEmpty ? "未记录模型名" : name
    }

    /// 金额那一栏：按同系列的标价估算的新模型前面加「≈」
    static func costModelMoney(_ m: CostModel, days: Bool) -> String {
        (m.pricedAs != nil ? "≈ " : "") + money(cents: m.cents(days: days))
    }

    /// 按模型那一块下面：有按同系列的标价估算的新模型时说一句
    static func costEstimated(_ rows: [CostModel], days: Bool) -> String? {
        let est = rows.filter { $0.pricedAs != nil && $0.name != nil && $0.cents(days: days) > 0 }
        guard let first = est.first, let model = first.name, let base = first.pricedAs else { return nil }
        if est.count == 1 { return "检测到新模型 \(model)：cca 的价格表中还没有它，暂按 \(base) 的价格估算（≈）" }
        return "检测到新模型 " + est.compactMap(\.name).joined(separator: "、") + "：cca 的价格表中还没有，暂按同系列的价格估算（≈）"
    }

    // MARK: 用量统计

    /// 平均每周用到多少：「63%」；一周都还没过完时是 nil
    static func weeksAverage(_ s: UsageWeeksSummary) -> String? {
        guard s.weeks > 0, let avg = s.averagePercent else { return nil }
        return "\(avg)%"
    }

    /// 几周用完、几周没用到一半（平均数下面那一行）；都没有时是空的
    static func weeksNotes(_ s: UsageWeeksSummary) -> [String] {
        (s.maxed > 0 ? ["\(s.maxed) 周用完"] : []) + (s.underHalf > 0 ? ["\(s.underHalf) 周不到一半"] : [])
    }

    /// 一组的名字（Mac App 的 Texts.department）
    static func department(_ tool: String) -> String {
        switch tool {
        case "claude": return "Claude 账号"
        case "codex": return "ChatGPT 账号"
        case "agy": return "Antigravity 账号"
        default: return toolName(tool) + " 账号"
        }
    }

    // MARK: 总览

    /// 电脑上开着什么：「Claude App 开着 · 终端里 2 个会话，已 12 分钟」（Mac App 的 Texts.working）
    static func working(_ a: AccountStatus, now: Double) -> String {
        var parts: [String] = []
        if a.appOpen { parts.append(appLabel(a.tool) + " 开着") }
        if a.cliSessions > 0 {
            var s = a.cliSessions == 1 ? "终端里在用" : "终端里 \(a.cliSessions) 个会话"
            if let since = a.cliSince { s += "，" + (now - since < 60 ? "刚开始" : "已 " + span(now - since)) }
            parts.append(s)
        }
        return parts.isEmpty ? "在用" : parts.joined(separator: " · ")
    }

    /// 工具的桌面 App 的叫法（Mac App 的 appLabel 没有 cca 给的名字时的那几个）
    static func appLabel(_ tool: String) -> String {
        tool == "codex" ? "ChatGPT App" : tool == "agy" ? "桌面 App" : "Claude App"
    }

    // MARK: 共享与同步（Mac App 的 teamState、teamServerUpdate、teamSummary）

    /// Team 那一行状态：照 cca 给的说（没同步成、被移除时 cca 给了原因）
    static func teamState(_ t: HostTeam) -> String {
        switch t.state {
        case "ok": return t.seq > 0 ? "配置已同步" : "已同步"
        case "pending": return "配置未同步"
        case "local": return "未连接同步服务器"
        case "removed": return nonEmpty(t.error) ?? "这台电脑已被管理员移除"
        case "lost": return nonEmpty(t.error) ?? "服务器上没有这台电脑的记录：请等管理员重新登记"
        default: return nonEmpty(t.error).map { "配置同步失败：\($0)" } ?? "配置同步失败"
        }
    }

    /// Team 的服务器要更新：少了什么，谁来更新
    static func teamServerUpdate(_ t: HostTeam) -> String? {
        guard let u = t.update else { return nil }
        let why = nonEmpty(u).map { "：\($0)" } ?? ""
        return t.isOwner ? "服务器需要更新\(why)。请在这台电脑上的 cca App 中更新（数据不变，更新时服务器会暂停几秒）"
            : "服务器需要更新\(why)。请联系创建这个 Team 的管理员更新服务器"
    }

    /// 这台电脑在里面叫什么、给了它几个账号、服务器（host 是 false 时不写服务器：写在 Team 那一块的标题下面了）
    static func teamSummary(_ t: HostTeam, host: Bool = true) -> String {
        var parts: [String] = []
        if let d = nonEmpty(t.device) { parts.append("设备名 \(d)") }
        parts.append(t.accounts > 0 ? "已分配 \(t.accounts) 个账号" : "未分配账号")
        if host, let h = nonEmpty(t.host) { parts.append(h) }
        return parts.joined(separator: " · ")
    }

    /// 管理员的标签：建 Team 的那台「管理员」，别的管理员授予的「管理员（受托）」；成员没有
    static func teamRole(_ t: HostTeam) -> String? {
        t.isAdmin ? (t.isOwner ? "管理员" : "管理员（受托）") : nil
    }
}
