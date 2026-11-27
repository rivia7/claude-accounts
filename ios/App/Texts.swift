import CcaRemoteKit
import Foundation

/// 界面上的文字：说法照 Mac App（macos/Sources/CcaKit/Model/Texts.swift 的「远程会话」那一段）：照实说、不用比喻，简短，
/// 用常见 App 的说法（未登录、无法连接、已断开）；套餐、产品名这些专有名词照原来的英文（Pro、Max、Team、API key）
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

    /// 会话的第二行：「~/srv/api · 已运行 2 小时 · 另有 1 处接入」
    static func sessionDetail(_ s: RemoteSession, now: Double) -> String {
        var parts: [String] = []
        if let d = nonEmpty(s.info.cwd) { parts.append(d) }
        if s.info.since > 0 { parts.append("已运行 " + span(now - s.info.since)) }
        if s.viewers > 0 { parts.append("另有 \(s.viewers) 处接入") }
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

    /// 分模型的每周额度下面那句话（Mac App 的 Texts.modelShare）：「Fable 最多可用本周额度的 50%，已用 9%」；它那份用完了：
    /// 「Fable 的额度已用完，其他模型还能用本周剩余的 49%」；cca 没认出份额时只说它单独计算额度
    static func modelShare(_ m: ModelWindow, weekly: UsageWindow?) -> String {
        let name = nonEmpty(m.name) ?? "这个模型"
        guard let allowance = m.allowancePercentOfWeekly else { return "\(name) 单独计算额度" }
        if !m.window.reset, (m.window.utilization ?? 0) >= 100 {
            let left = weekly.flatMap { w in w.reset ? nil : w.utilization.map { 100 - $0.rounded() } }
            return "\(name) 的额度已用完" + (left.flatMap { $0 > 0 ? "，其他模型还能用本周剩余的 \(percent($0))" : nil } ?? "")
        }
        return "\(name) 最多可用本周额度的 \(percent(allowance))" + (m.usedPercentOfWeekly.map { "，已用 \(percent($0))" } ?? "")
    }

    /// 建议先用（cca 给的 use_first）：「本周额度还剩 59%，明天 19:55 重置，建议先用这个账号」
    static func useFirst(_ f: UseFirst, now: Double) -> String {
        let when = UsageText.backTime(epoch: f.untilEpoch, now: now, today: "今天 ")
        return "本周额度还剩 \(f.leftPercent)%，\(when) 重置，建议先用这个账号"
    }

    /// 读屏念的套餐：“Ultra 套餐”。套餐照 cca 给的原名（Pro、Max、Plus……），档位不起中文的叫法：界面上靠徽记的样子区分
    static func planName(_ plan: String) -> String {
        "\(plan) 套餐"
    }

    /// 查用量没成的原因码（PROTOCOL 3.1 的 error_code）→ 一句话（和 Mac App 的 Texts.usageError 一样）
    static func usageError(_ code: String) -> String {
        switch code {
        case "login": return "未登录"
        case "mismatch": return "登录的不是这个账号"
        case "route": return "没有可用的出口"
        case "port": return "端口无法连接"
        case "region": return "出口位于不支持的地区"
        case "trace": return "无法检测出口"
        case "unsupported": return "当前版本不支持查询用量"
        case "timeout": return "查询超时"
        case "nolimits": return "未获取到用量数据"
        case "auth": return "登录已失效"
        default: return "查询失败"
        }
    }

    /// 用量是什么时候的（账号页「用量」右边）：10 分钟内查到的写「3 分钟前查询」，否则「缓存，2 小时前更新」，过时了的「数据已过时，…」
    static func freshness(_ u: AccountUsage, now: Double) -> (text: String, fresh: Bool) {
        let age = u.fetchedEpoch.map { now - $0 }
        if u.source == "live", let age, age < 600, !u.stale { return (agoPhrase(age) + "查询", true) }
        let when = age.map { "，" + agoPhrase($0) + "更新" } ?? ""
        return ((u.stale ? "数据已过时" : "缓存") + when, false)
    }

    /// 一个账号的那句话（Mac App 的 Texts.mood）：在用时有问题，问题跟在后面（「在用 · 额度已用完，16:00 恢复」）
    static func mood(_ m: AccountMood, now: Double, brief: Bool = true) -> String {
        let problem = m.trouble.map { trouble($0, now: now, brief: brief) }
        if m.busy { return "在用" + (problem.map { " · " + $0 } ?? "") }
        return problem ?? "空闲"
    }

    static func trouble(_ t: AccountTrouble, now: Double, brief: Bool = true) -> String {
        switch t {
        case .loggedOut: return "未登录"
        case .wrongAccount: return "登录的不是这个账号"
        case .expired: return brief ? "登录已失效" : "登录已失效，请在电脑上重新登录"
        case .usedUp(let until): return "额度已用完" + (until.map { "，" + UsageText.backAt(epoch: $0, now: now) } ?? "")
        }
    }

    /// 账号是哪个工具的：「账号」页上面的工具切换、页头那一句（和 Mac App 侧栏的工具切换一样叫法：Claude、ChatGPT、Antigravity）
    static func toolName(_ tool: String) -> String {
        switch tool {
        case "claude": return "Claude"
        case "codex": return "ChatGPT"
        case "agy": return "Antigravity"
        default: return tool
        }
    }

    /// 「账号」页顶上那一行（Mac App 的 officeSummary）：列着的这些账号（这个工具的，筛着套餐时只算这个套餐的）
    /// 「4 个 Claude 账号 · 1 个在用 · 2 个空闲 · 1 个登录失效」（0 的不写）
    static func census(_ c: AccountCensus, tool: String) -> String {
        if c.total == 0 { return "还没有账号" }
        let rest = [(c.working, "在用"), (c.idle, "空闲"), (c.resting, "额度用完"), (c.absent, "未登录"), (c.wrongPerson, "账号不符"),
                    (c.expired, "登录失效")]
        return (["\(c.total) 个 \(toolName(tool)) 账号"] + rest.filter { $0.0 > 0 }.map { "\($0.0) 个\($0.1)" }).joined(separator: " · ")
    }

    /// 在那台电脑上用这个账号开一个会话的命令
    static func launchCommand(_ a: AccountStatus) -> String {
        let prefix = a.tool == "codex" ? "cca codex" : a.tool == "agy" ? "cca agy" : "cca"
        return "\(prefix) @\(nonEmpty(a.name) ?? "账号")"
    }

    // MARK: 每周用量（cca 给的 usage_weeks，说法和 Mac App 的一样）

    /// 「8 周平均用了 63% · 2 周用完 · 3 周不到一半」；一周都还没过完时是 nil
    static func weeksSummary(_ s: UsageWeeksSummary) -> String? {
        guard s.weeks > 0, let avg = s.averagePercent else { return nil }
        let head = s.weeks == 1 ? "1 周用了 \(avg)%" : "\(s.weeks) 周平均用了 \(avg)%"
        let notes = (s.maxed > 0 ? ["\(s.maxed) 周用完"] : []) + (s.underHalf > 0 ? ["\(s.underHalf) 周不到一半"] : [])
        return ([head] + notes).joined(separator: " · ")
    }

    /// 一周的细节（点了那根柱子、读屏）：「11 月 6 日–13 日：用了 87%，期间重置过 1 次」；还没过完的这一周：「本周已用 20%，周四 10:00 重置」
    static func weekDetail(_ w: UsageWeek, now: Double) -> String {
        let resets = w.resets > 0 ? "期间重置过 \(w.resets) 次" : nil
        if w.current {
            return "本周已用 \(percent(w.usedPercent))" + (resets.map { "（\($0)）" } ?? "") + "，" + resetText(epoch: w.endEpoch, now: now)
        }
        return weekRange(w, now: now) + "：用了 \(percent(w.usedPercent))" + (resets.map { "，" + $0 } ?? "")
            + (w.partial ? "；最后一天没有数据，实际可能更多" : "")
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

    /// 「从 11 月 11 日开始记录」
    static func weeksSince(_ epoch: Double, now: Double) -> String {
        "从 " + dayText(epoch, now: now) + "开始记录"
    }

    // MARK: 出了什么事

    /// 配对没成：RemoteFailure 的 code → 一句话
    static func pairFailure(_ code: String) -> String {
        switch code {
        case "pin": return "服务器证书与二维码不符，可能连到了其他服务器。请换个网络重新扫码；如果仍然不行，请联系服务器的管理员。"
        case "unreachable": return "无法连接服务器。请检查网络后重新扫码。"
        case "no_pairing": return "二维码已使用或已过期。请在电脑上重新点「和手机配对…」，扫描新的二维码。"
        case "no_answer": return "电脑无响应。请确认电脑上的配对窗口仍然开着，然后重新点「和手机配对…」并扫码。"
        case "bad_peer": return "验证失败，未完成配对。请重新扫码。"
        case "rate", "busy": return "服务器繁忙，请稍后再试。"
        case "clock": return "手机时间与服务器相差太多。请在「设置」→「通用」→「日期与时间」中打开「自动设置」。"
        case "edges": return "配对的设备已达上限（每台最多 16 个）。请先在电脑上取消不用的配对。"
        case "mismatch": return "两端的配对信息不一致，未完成配对。请在电脑上重新点「和手机配对…」并扫码。"
        case "full": return "服务器存储已满，请联系服务器的管理员。"
        default: return "配对失败（\(code)），请重新扫码。"
        }
    }

    /// 没列出会话：一台电脑所在的服务器没问到
    static func listFailure(_ code: String) -> String {
        switch code {
        case "pin": return "服务器证书与配对时的不一致"
        case "unreachable": return "无法连接服务器"
        case "denied", "unknown_device": return "服务器不再识别这台手机，配对可能已取消"
        case "no_key": return "手机上缺少这个配对的密钥，请重新配对"
        case "rate", "busy": return "服务器繁忙"
        case "clock": return "手机时间与服务器相差太多"
        default: return "查询失败（\(code)）"
        }
    }

    /// 终端那一页顶上的那句话：连接的状态
    static func linkState(_ s: Attachment.State) -> String? {
        switch s {
        case .connecting: return "正在接入…"
        case .open: return nil
        case .retrying: return "连接已断开，正在重新连接…"
        case .closed(let why): return closed(why)
        }
    }

    static func closed(_ why: String) -> String {
        switch why {
        case "detached": return "已断开，会话仍在电脑上运行"
        case "ended": return "会话已结束"
        case "gone": return "找不到这个会话（可能已结束）"
        case "unpaired": return "对方已取消配对，无法接入"
        case "gave_up": return "连接已断开，重新连接失败"
        case "refused": return "电脑不允许这台手机接入这个会话"
        case "version": return "两边的 cca 版本不兼容，请都更新到最新版"
        case "auth": return "无法确认对方是配对的电脑，已断开（未发送任何内容）"
        case "nohost": return "电脑暂时没有连接服务器"
        case "unreachable": return "无法连接服务器的数据端口"
        case "busy", "rate", "sessions": return "服务器繁忙，请稍后再试"
        case "ticket": return "服务器拒绝了这次接入，请重试"
        default: return "接入失败（\(why)）"
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
        case "denied": return "电脑不允许这台手机远程启动会话。请在 Mac 上打开 cca App 的「远程会话」，在「已配对的机器」中为这台手机打开「允许远程启动」（或在终端运行 cca remote allow 手机名称 launch on）"
        case "no_template": return "这个模板已被删除。列表已刷新，请重新选择"
        case "changed": return "这个模板已被修改。列表已刷新，请确认后再启动"
        case "no_account": return "模板使用的账号已不存在，请在电脑上修改这个模板"
        case "no_dir": return "模板的文件夹已不存在，请在电脑上修改这个模板"
        case "bad_args": return "模板的参数无法使用（未通过启动前的检查），请在电脑上修改这个模板"
        case "other_hub": return "模板的账号现在经另一个 Team 的服务器中转，无法从这里启动，请使用经那个 Team 的配对"
        case "busy": return "电脑上打开的会话太多，请先结束几个再试"
        case "rate": return "启动太频繁，请稍后再试"
        case "spawn": return "会话启动失败。可在电脑上运行 cca remote launcher log 查看原因"
        case "bad": return "电脑无法识别这个请求，请把本 App 和电脑上的 cca 都更新到最新版"
        default: return nonEmpty(text) ?? "启动失败（\(code)）"
        }
    }

    /// 没等到启动器回话：原因码（RemoteFailure 的、连接关掉的那几个词，另有 timeout、bad_ask，CcaRemoteKit 的 LaunchResult）→
    /// 一句话。请求发出去以后断了、没等到回话的，会话可能已经起来了
    static func launchUnreached(_ code: String) -> String {
        switch code {
        case "no_session", "nohost": return "电脑暂时收不到启动请求：可能已休眠、断网，或远程启动的后台进程没有运行"
        case "timeout", "lost": return "电脑没有响应。会话可能已经启动，请稍后在列表中查看"
        case "bad_ask": return "开场白无法发送：不能只有空白，最多 500 个字"
        case "cancelled": return "已取消等待"
        case "refused": return "电脑不允许这台手机远程启动会话"
        case "pin", "unreachable", "denied", "unknown_device", "no_key", "rate", "busy", "clock": return listFailure(code)
        default: return closed(code)
        }
    }
}
