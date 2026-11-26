import CcaRemoteKit
import Foundation

/// 界面上的文字：说法照 Mac App（macos/Sources/CcaKit/Model/Texts.swift 的「远程会话」那一段），照实说、不用比喻
enum Texts {
    static func nonEmpty(_ s: String?) -> String? {
        guard let s, !s.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        return s
    }

    /// 别的机器上的程序写的一行字（终端的标题）显示之前：控制字符、看不见的排版字符去掉，截短（CcaRemoteKit 的 plainLine：
    /// 通知扩展弹推来的通知也用它）
    static func plainLine(_ s: String, max: Int) -> String {
        CcaRemoteKit.plainLine(s, max: max)
    }

    /// 会话里跑的是哪个命令行工具
    static func cliProduct(_ tool: String) -> String? {
        switch tool {
        case "claude": return "Claude Code"
        case "codex": return "Codex"
        case "agy": return "Antigravity"
        case "": return nil
        default: return tool
        }
    }

    /// 一个会话的名字：「Claude Code · 工作」
    static func sessionTitle(_ s: SessionInfo) -> String {
        [cliProduct(s.tool), nonEmpty(s.name)].compactMap { $0 }.joined(separator: " · ")
    }

    static func sessionTitle(_ s: RemoteSession) -> String {
        nonEmpty(sessionTitle(s.info)) ?? "会话 \(s.sid.prefix(6))"
    }

    /// 会话的第二行：「~/srv/api · 开了 2 小时 · 另有 1 处接着」
    static func sessionDetail(_ s: RemoteSession, now: Double) -> String {
        var parts: [String] = []
        if let d = nonEmpty(s.info.cwd) { parts.append(d) }
        if s.info.since > 0 { parts.append("开了 " + span(now - s.info.since)) }
        if s.viewers > 0 { parts.append("另有 \(s.viewers) 处接着") }
        return parts.joined(separator: " · ")
    }

    static func span(_ seconds: Double) -> String {
        let m = Int(max(seconds, 0) / 60)
        if m < 1 { return "不到 1 分钟" }
        if m < 60 { return "\(m) 分钟" }
        let h = m / 60, mm = m % 60
        if h < 24 { return mm > 0 ? "\(h) 小时 \(mm) 分" : "\(h) 小时" }
        let d = h / 24, hh = h % 24
        return hh > 0 ? "\(d) 天 \(hh) 小时" : "\(d) 天"
    }

    static func agoPhrase(_ seconds: Double) -> String {
        let m = Int(max(seconds, 0) / 60)
        if seconds < 60 { return "刚刚" }
        if m < 60 { return "\(m) 分钟前" }
        if m < 1440 { return "\(m / 60) 小时前" }
        return "\(m / 1440) 天前"
    }

    /// 配对过的电脑的第二行：「11 月 3 日配对 · 示例 Team」
    static func peerDetail(_ p: Peer, now: Double) -> String {
        let c = Calendar.current.dateComponents([.year, .month, .day], from: Date(timeIntervalSince1970: p.at))
        let thisYear = Calendar.current.component(.year, from: Date(timeIntervalSince1970: now))
        let day = c.year == thisYear ? "\(c.month ?? 0) 月 \(c.day ?? 0) 日" : "\(c.year ?? 0) 年 \(c.month ?? 0) 月 \(c.day ?? 0) 日"
        return [day + "配对", p.team].compactMap { $0 }.joined(separator: " · ")
    }

    // MARK: 账号（说法照 Mac App 的总览、账号页：macos/Sources/CcaKit/Model/Texts.swift，照实说、不用比喻）

    static func percent(_ p: Double) -> String {
        "\(Int(p.rounded()))%"
    }

    /// 重置时间：一天以内写还有多久（2 小时 13 分后重置），再远写星期几、几点（周二 09:00 重置）
    static func resetText(epoch: Double, now: Double) -> String {
        if epoch <= now { return "已重置" }
        if epoch - now < 86400 { return span(epoch - now) + "后重置" }
        return UsageText.weekdayTime(epoch) + " 重置"
    }

    /// “11 月 3 日”（不是今年的带年份：“2025 年 10 月 3 日”）
    static func dayText(_ epoch: Double, now: Double) -> String {
        let cal = Calendar.current
        let c = cal.dateComponents([.year, .month, .day], from: Date(timeIntervalSince1970: epoch))
        let year = cal.component(.year, from: Date(timeIntervalSince1970: now))
        let day = "\(c.month ?? 0) 月 \(c.day ?? 0) 日"
        return c.year == year ? day : "\(c.year ?? 0) 年 " + day
    }

    /// 什么时候过期（重置额度的机会）：“11 月 20 日过期”，一天以内写还有多久（“5 小时 3 分后过期”）
    static func expiryText(epoch: Double, now: Double) -> String {
        if epoch <= now { return "已过期" }
        if epoch - now < 86400 { return span(epoch - now) + "后过期" }
        return dayText(epoch, now: now) + "过期"
    }

    /// 还能重置几次额度（Codex）：「还能重置额度 2 次 · 最早一次 11 月 24 日过期」（只有一次时不说最早）
    static func resetCredits(_ rc: ResetCredits, now: Double) -> String {
        let n = rc.availableCount ?? 0
        var s = "还能重置额度 \(n) 次"
        if n > 0, let e = rc.nextExpiresEpoch { s += " · " + (n > 1 ? "最早一次 " : "") + expiryText(epoch: e, now: now) }
        return s
    }

    /// 分模型的每周额度下面那句话（Mac App 的 Texts.modelShare）：「Fable 最多占本周额度的 50%，已占 9%」；它那份用完了：
    /// 「Fable 的份额用完了，别的模型还能用本周剩下的 49%」；cca 没认出份额时只说它的额度单独计
    static func modelShare(_ m: ModelWindow, weekly: UsageWindow?) -> String {
        let name = nonEmpty(m.name) ?? "这个模型"
        guard let allowance = m.allowancePercentOfWeekly else { return "\(name) 的额度单独计" }
        if !m.window.reset, (m.window.utilization ?? 0) >= 100 {
            let left = weekly.flatMap { w in w.reset ? nil : w.utilization.map { 100 - $0.rounded() } }
            return "\(name) 的份额用完了" + (left.flatMap { $0 > 0 ? "，别的模型还能用本周剩下的 \(percent($0))" : nil } ?? "")
        }
        return "\(name) 最多占本周额度的 \(percent(allowance))" + (m.usedPercentOfWeekly.map { "，已占 \(percent($0))" } ?? "")
    }

    /// 建议先用（cca 给的 use_first）：「本周额度还剩 59%，明天 19:55 重置，建议先用这个账号」
    static func useFirst(_ f: UseFirst, now: Double) -> String {
        let when = UsageText.backTime(epoch: f.untilEpoch, now: now, today: "今天 ")
        return "本周额度还剩 \(f.leftPercent)%，\(when) 重置，建议先用这个账号"
    }

    /// 套餐档位的叫法（读屏用；界面上靠徽记的样子区分）
    static func planTier(_ t: String) -> String? {
        switch t {
        case "entry": return "入门"
        case "standard": return "标准"
        case "advanced": return "进阶"
        case "premium": return "尊贵"
        default: return nil
        }
    }

    /// 读屏念的套餐：“Ultra，尊贵套餐”
    static func planName(_ plan: String, tier: String) -> String {
        planTier(tier).map { "\(plan)，\($0)套餐" } ?? plan
    }

    /// 查用量没成的原因码（PROTOCOL 3.1 的 error_code）→ 一句话（和 Mac App 的 Texts.usageError 一样）
    static func usageError(_ code: String) -> String {
        switch code {
        case "login": return "没登录"
        case "mismatch": return "登录的不是这个账号"
        case "route": return "没有可用的出口"
        case "port": return "端口连不上"
        case "region": return "出口在不可用地区"
        case "trace": return "查不到出口"
        case "unsupported": return "这个版本不支持查用量"
        case "timeout": return "查询超时"
        case "nolimits": return "没有拿到用量数据"
        case "auth": return "登录已失效"
        default: return "查询失败"
        }
    }

    /// 用量是什么时候的（账号页「用量」右边）：10 分钟内查到的写「3 分钟前查询」，否则「缓存，2 小时前更新」，过时了的「数据已经过时，…」
    static func freshness(_ u: AccountUsage, now: Double) -> (text: String, fresh: Bool) {
        let age = u.fetchedEpoch.map { now - $0 }
        if u.source == "live", let age, age < 600, !u.stale { return (agoPhrase(age) + "查询", true) }
        let when = age.map { "，" + agoPhrase($0) + "更新" } ?? ""
        return ((u.stale ? "数据已经过时" : "缓存") + when, false)
    }

    /// 一个账号的那句话（Mac App 的 Texts.mood）：在用时有问题，问题跟在后面（「在用 · 额度用完了，16:00 恢复」）
    static func mood(_ m: AccountMood, now: Double, brief: Bool = true) -> String {
        let problem = m.trouble.map { trouble($0, now: now, brief: brief) }
        if m.busy { return "在用" + (problem.map { " · " + $0 } ?? "") }
        return problem ?? "空闲"
    }

    static func trouble(_ t: AccountTrouble, now: Double, brief: Bool = true) -> String {
        switch t {
        case .loggedOut: return "还没登录"
        case .wrongAccount: return "登录的不是这个账号"
        case .expired: return brief ? "登录已失效" : "登录已失效，要在那台电脑上重新登录"
        case .usedUp(let until): return "额度用完了" + (until.map { "，" + UsageText.backAt(epoch: $0, now: now) } ?? "")
        }
    }

    /// 「账号」页顶上那一行（Mac App 的 officeSummary）：「8 个账号 · 2 个在用 · 1 个额度用完 · 1 个没登录」（0 的不写）
    static func census(_ c: AccountCensus) -> String {
        if c.total == 0 { return "还没有账号" }
        let rest = [(c.working, "在用"), (c.idle, "空闲"), (c.resting, "额度用完"), (c.absent, "没登录"), (c.wrongPerson, "登错号"),
                    (c.expired, "登录失效")]
        return (["\(c.total) 个账号"] + rest.filter { $0.0 > 0 }.map { "\($0.0) 个\($0.1)" }).joined(separator: " · ")
    }

    /// 在那台电脑上用这个账号开一个会话的命令
    static func launchCommand(_ a: AccountStatus) -> String {
        let prefix = a.tool == "codex" ? "cca codex" : a.tool == "agy" ? "cca agy" : "cca"
        return "\(prefix) @\(nonEmpty(a.name) ?? "账号")"
    }

    // MARK: 每周用量（cca 给的 usage_weeks，说法和 Mac App 的一样）

    /// 「记下的 8 周平均用到 63% · 2 周用完 · 3 周没用到一半」；一周都还没过完时是 nil
    static func weeksSummary(_ s: UsageWeeksSummary) -> String? {
        guard s.weeks > 0, let avg = s.averagePercent else { return nil }
        let head = s.weeks == 1 ? "记下的 1 周用到 \(avg)%" : "记下的 \(s.weeks) 周平均用到 \(avg)%"
        let notes = (s.maxed > 0 ? ["\(s.maxed) 周用完"] : []) + (s.underHalf > 0 ? ["\(s.underHalf) 周没用到一半"] : [])
        return ([head] + notes).joined(separator: " · ")
    }

    /// 一周的细节（点了那根柱子、读屏）：「11 月 6 日–13 日：用到 87%，中间重置过 1 次」；还没过完的这一周：「本周到现在用了 20%，周四 10:00 重置」
    static func weekDetail(_ w: UsageWeek, now: Double) -> String {
        let resets = w.resets > 0 ? "中间重置过 \(w.resets) 次" : nil
        if w.current {
            return "本周到现在用了 \(percent(w.usedPercent))" + (resets.map { "（\($0)）" } ?? "") + "，" + resetText(epoch: w.endEpoch, now: now)
        }
        return weekRange(w, now: now) + "：用到 \(percent(w.usedPercent))" + (resets.map { "，" + $0 } ?? "")
            + (w.partial ? "；结束前一天没查到，实际可能更多" : "")
    }

    /// 「11 月 6 日–13 日」、「10 月 30 日–11 月 6 日」
    static func weekRange(_ w: UsageWeek, now: Double) -> String {
        let cal = Calendar.current
        let a = cal.dateComponents([.year, .month], from: Date(timeIntervalSince1970: w.startEpoch))
        let b = cal.dateComponents([.year, .month, .day], from: Date(timeIntervalSince1970: w.endEpoch))
        let start = dayText(w.startEpoch, now: now)
        if a.year == b.year && a.month == b.month { return start + "–\(b.day ?? 0) 日" }
        return start + "–" + dayText(w.endEpoch, now: now)
    }

    /// 柱子下面的日期：这一周从哪天开始（「11/6」）；还没过完的写「本周」
    static func weekLabel(_ w: UsageWeek) -> String {
        if w.current { return "本周" }
        let c = Calendar.current.dateComponents([.month, .day], from: Date(timeIntervalSince1970: w.startEpoch))
        return "\(c.month ?? 0)/\(c.day ?? 0)"
    }

    /// 「从 11 月 11 日开始记」
    static func weeksSince(_ epoch: Double, now: Double) -> String {
        "从 " + dayText(epoch, now: now) + "开始记"
    }

    // MARK: 出了什么事

    /// 配对没成：RemoteFailure 的 code → 一句话
    static func pairFailure(_ code: String) -> String {
        switch code {
        case "pin": return "服务器的证书和二维码里的对不上：可能连到了别的服务器。换个网络再扫一次；还是这样，请那台电脑的主人看看服务器。"
        case "unreachable": return "连不上服务器。看看网络，再扫一次。"
        case "no_pairing": return "这个二维码已经用过或者过期了。在电脑上重新点「和手机配对…」，扫新的二维码。"
        case "no_answer": return "那台电脑一直没有回应。电脑上的配对窗口还开着吗？重新点「和手机配对…」再扫一次。"
        case "bad_peer": return "对方的回答验证不过，没有配对。重新扫一次。"
        case "rate", "busy": return "服务器这会儿太忙，过一会儿再试。"
        case "clock": return "这台手机的时间和服务器差太多。打开「设置」→「通用」→「日期与时间」里的「自动设置」。"
        case "edges": return "配对的设备太多了（一台最多 16 个），先在电脑上取消几个不用的配对。"
        case "mismatch": return "两边算出来的配对对不上，没有配对。在电脑上重新点「和手机配对…」再扫一次。"
        case "full": return "服务器存不下了，请那台电脑的主人看看服务器。"
        default: return "没配对成功（\(code)）。重新扫一次。"
        }
    }

    /// 没列出会话：一台电脑所在的服务器没问到
    static func listFailure(_ code: String) -> String {
        switch code {
        case "pin": return "服务器的证书变了，和配对时记下的对不上"
        case "unreachable": return "连不上服务器"
        case "denied", "unknown_device": return "服务器不认这台手机了：配对可能已经取消"
        case "no_key": return "这台手机上没有这个配对的密钥，要重新配对"
        case "rate", "busy": return "服务器这会儿太忙"
        case "clock": return "这台手机的时间和服务器差太多"
        default: return "没问到（\(code)）"
        }
    }

    /// 终端那一页顶上的那句话：连接的状态
    static func linkState(_ s: Attachment.State) -> String? {
        switch s {
        case .connecting: return "正在接入…"
        case .open: return nil
        case .retrying: return "连接断了，正在重新接上…"
        case .closed(let why): return closed(why)
        }
    }

    static func closed(_ why: String) -> String {
        switch why {
        case "detached": return "已断开，会话在那边照常运行"
        case "ended": return "会话已经结束"
        case "gone": return "没有这个会话了（可能已经结束）"
        case "unpaired": return "那台电脑取消了配对，接入不了了"
        case "gave_up": return "连接断了，一直没有再接上"
        case "refused": return "那台电脑不让这台手机接入这个会话"
        case "version": return "那台电脑上的 cca 版本不一样，先把两边都更新"
        case "auth": return "对方证明不了自己是配对的那台电脑：已经断开，什么都没发给它"
        case "nohost": return "那台电脑这会儿没连着服务器"
        case "unreachable": return "连不上服务器的数据端口"
        case "busy", "rate", "sessions": return "服务器这会儿太忙，过一会儿再试"
        case "ticket": return "服务器没认这次接入，再试一次"
        default: return "没接上（\(why)）"
        }
    }

    // MARK: 新建会话（远程启动）

    /// 模板的第二行：「Claude Code · 工作」
    static func templateTool(_ t: LaunchTemplate) -> String {
        [cliProduct(t.tool), nonEmpty(t.account)].compactMap { $0 }.joined(separator: " · ")
    }

    /// 那台电脑的启动器说没起：原因码（server/README.md「启动器」）→ App 自己的一句话；不认识的原因码照那台电脑写的原因说，
    /// 它也没写的说一句通用的
    static func launchFailure(code: String, text: String) -> String {
        switch code {
        case "denied": return "那台电脑现在不让这台手机远程启动会话。在 Mac 上打开 cca App 的「远程会话」，在「配对过的机器」里打开这台手机的「允许远程启动」（终端里是 cca remote allow 这台手机的名字 launch on）"
        case "no_template": return "那台电脑上已经没有这个模板了。列表刷新过了，重新选一个"
        case "changed": return "这个模板在那台电脑上改过了。列表刷新过了，看一眼再启动"
        case "no_account": return "模板用的账号在那台电脑上已经没有了，要在那台电脑上改一下这个模板"
        case "no_dir": return "模板的文件夹在那台电脑上已经没有了，要在那台电脑上改一下这个模板"
        case "bad_args": return "模板的参数在那台电脑上用不了（启动前的检查没过），要在那台电脑上改一下这个模板"
        case "other_hub": return "模板的账号现在走别的 Team 的服务器，这里起不了：从经那个 Team 的配对起"
        case "busy": return "那台电脑上开着的会话太多了，先结束几个再试"
        case "rate": return "启动得太频繁了，过一会儿再试"
        case "spawn": return "会话没起来。在那台电脑上运行 cca remote launcher log 看看原因"
        case "bad": return "那台电脑看不懂这个请求：把这个 App 和那台电脑上的 cca 都更新到最新"
        default: return nonEmpty(text) ?? "没启动成功（\(code)）"
        }
    }

    /// 没等到启动器回话：原因码（RemoteFailure 的、连接关掉的那几个词，另有 timeout、bad_ask，CcaRemoteKit 的 LaunchResult）→
    /// 一句话。请求发出去以后断了、没等到回话的，会话可能已经起来了
    static func launchUnreached(_ code: String) -> String {
        switch code {
        case "no_session", "nohost": return "那台电脑这会儿收不到启动的请求：它睡着了、没联网，或者远程启动的后台进程没在运行"
        case "timeout", "lost": return "没等到那台电脑回话。会话可能已经起来了，过一会儿在列表里看看"
        case "bad_ask": return "开场白发不了：不能只有空白，最多 500 个字"
        case "cancelled": return "没等那台电脑回话"
        case "refused": return "那台电脑不让这台手机远程启动会话"
        case "pin", "unreachable", "denied", "unknown_device", "no_key", "rate", "busy", "clock": return listFailure(code)
        default: return closed(code)
        }
    }
}
