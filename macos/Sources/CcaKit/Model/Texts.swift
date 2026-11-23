import Foundation

// 界面上的文字和判断（Texts、UsageLevel、Remedy、Attention；只依赖 Foundation，方便测试）。不重算 cca 的规则：只把 cca 给的码换成中文、按时间格式化。

/// 用量的颜色档：<70 正常，70–89 偏高，≥90 快用完。
enum UsageLevel: Sendable, Equatable {
    case normal, warning, critical

    init(percent: Double) {
        if percent >= 90 {
            self = .critical
        } else if percent >= 70 {
            self = .warning
        } else {
            self = .normal
        }
    }
}

/// 失败时给的补救按钮（PROTOCOL 4.3 第 5 条）。
enum Remedy: Sendable, Equatable {
    case none
    case routerSync
    case handoff(Handoff)
    case openURL(String)

    static let claudeDownload = "https://claude.ai/download"

    /// 按 fail 的 code 给补救（prefix 默认用工具的前缀；AppModel.remedy(for:) 传 state 里的 cli_prefix）。
    static func forCode(_ code: FailCode, tool: ToolID, key: String?, prefix: [String]? = nil) -> Remedy {
        let p = prefix ?? tool.defaultPrefix
        switch code {
        case .portClosed:
            return .routerSync
        case .tableInvalid, .nodeMissing:
            return .handoff(.edit(prefix: p))
        case .noRouter:
            return .handoff(.routerSetup)
        case .localUnavailable:
            return .handoff(.localSetup)
        case .notLoggedIn, .loginMismatch:
            guard let key else { return .none }
            return .handoff(.login(prefix: p, key: key, name: key))
        case .appMissing:
            // ChatGPT App（Codex 的桌面版）没有固定的下载地址：cca 的说明里写着用 codex app 打开它的安装页；Antigravity 没有桌面 App
            return tool == .claude ? .openURL(claudeDownload) : .none
        case .toolMissing:
            // 辅助安装（cca 够新时才有，AppModel.remedy(for:) 按 hello.install_cli 把关）
            return .handoff(.installCLI(prefix: p, tool: tool))
        default:
            return .none
        }
    }

    var title: String? {
        switch self {
        case .none: return nil
        case .routerSync: return "同步路由器"
        case .handoff(let h): return h.inApp ? h.title + "…" : "在终端里运行 \(h.display)"
        case .openURL: return "去下载"
        }
    }
}

enum Texts {
    static let weekdays = ["日", "一", "二", "三", "四", "五", "六"]

    /// 一段时长：不到 1 分钟 / 25 分钟 / 2 小时 13 分 / 3 天 4 小时（和 cca 的 span 一样）。
    static func span(_ seconds: Double) -> String {
        let m = Int(max(seconds, 0) / 60)
        if m < 1 { return "不到 1 分钟" }
        if m < 60 { return "\(m) 分钟" }
        let h = m / 60, mm = m % 60
        if h < 24 { return mm > 0 ? "\(h) 小时 \(mm) 分" : "\(h) 小时" }
        let d = h / 24, hh = h % 24
        return hh > 0 ? "\(d) 天 \(hh) 小时" : "\(d) 天"
    }

    /// 多久以前：3 分钟 / 2 小时 / 4 天（和 cca 的 ago 一样，至少 1 分钟）。
    static func ago(_ seconds: Double) -> String {
        let m = Int(max(seconds, 0) / 60)
        if m < 60 { return "\(max(m, 1)) 分钟" }
        if m < 1440 { return "\(m / 60) 小时" }
        return "\(m / 1440) 天"
    }

    /// “刚刚” / “3 分钟前”。
    static func agoPhrase(_ seconds: Double) -> String {
        seconds < 60 ? "刚刚" : ago(seconds) + "前"
    }

    /// 重置时间：一天以内写还有多久（2 小时 13 分后重置），再远写星期几、几点（周二 09:00 重置）。
    static func resetText(epoch: Double, now: Double, calendar: Calendar = .current) -> String {
        if epoch <= now { return "已重置" }
        if epoch - now < 86400 { return span(epoch - now) + "后重置" }
        return weekdayTime(epoch, calendar: calendar) + " 重置"
    }

    /// “周二 09:00”
    static func weekdayTime(_ epoch: Double, calendar: Calendar) -> String {
        let c = calendar.dateComponents([.weekday], from: Date(timeIntervalSince1970: epoch))
        return "周" + weekdays[((c.weekday ?? 1) - 1 + 7) % 7] + " " + clock(epoch, calendar: calendar)
    }

    /// “09:00”
    static func clock(_ epoch: Double, calendar: Calendar) -> String {
        let c = calendar.dateComponents([.hour, .minute], from: Date(timeIntervalSince1970: epoch))
        return String(format: "%02d:%02d", c.hour ?? 0, c.minute ?? 0)
    }

    static func percent(_ p: Double) -> String {
        "\(Int(p.rounded()))%"
    }

    /// 过去的某个时刻：今天的写几点（14:02），昨天的写“昨天 18:40”，一周内写星期几（周二 09:05），再早写日期（11 月 3 日，不是今年的带年份）。
    static func pastTime(_ epoch: Double, now: Double, calendar: Calendar = .current) -> String {
        let d = Date(timeIntervalSince1970: epoch), n = Date(timeIntervalSince1970: now)
        let days = calendar.dateComponents([.day], from: calendar.startOfDay(for: d), to: calendar.startOfDay(for: n)).day ?? 0
        if days <= 0 { return clock(epoch, calendar: calendar) }
        if days == 1 { return "昨天 " + clock(epoch, calendar: calendar) }
        if days < 7 { return weekdayTime(epoch, calendar: calendar) }
        return dayText(epoch, now: now, calendar: calendar)
    }

    /// “11 月 3 日”（不是今年的带年份：“2025 年 10 月 3 日”）
    static func dayText(_ epoch: Double, now: Double, calendar: Calendar) -> String {
        let c = calendar.dateComponents([.year, .month, .day], from: Date(timeIntervalSince1970: epoch))
        let day = "\(c.month ?? 0) 月 \(c.day ?? 0) 日"
        return c.year == calendar.component(.year, from: Date(timeIntervalSince1970: now)) ? day : "\(c.year ?? 0) 年 " + day
    }

    /// 一台设备开着什么：“命令行里 2 个会话、Claude App 开着”
    static func deviceOpen(_ d: DeviceRow, appLabel: String) -> String {
        var parts: [String] = []
        if d.cli > 0 { parts.append("终端里 \(d.cli) 个会话") }
        if d.app { parts.append("\(appLabel) 开着") }
        return parts.joined(separator: "、")
    }

    /// 账号页「设备」里一台设备的状态（和 cca devices 的说法一样）：
    /// 在用 · 命令行里 2 个会话 · 14:02 起 / 开着，闲置 23 分钟 · 命令行里 1 个会话 / 离线 · 上次使用 10:20 / 没在用 · 上次使用 昨天 18:40
    static func deviceState(_ d: DeviceRow, appLabel: String, now: Double, calendar: Calendar = .current) -> String {
        let open = deviceOpen(d, appLabel: appLabel)
        let used = d.lastUsedEpoch.map { " · 上次使用 " + pastTime($0, now: now, calendar: calendar) } ?? ""
        switch d.state {
        case .inUse:
            return "在用" + (open.isEmpty ? "" : " · " + open) + (d.sinceEpoch.map { " · " + pastTime($0, now: now, calendar: calendar) + " 起" } ?? "")
        case .idle:
            return "开着，" + (d.lastActiveEpoch.map { "闲置 " + span(now - $0) } ?? "闲置") + (open.isEmpty ? "" : " · " + open)
        case .offline:
            return "离线" + used
        default:
            return "没在用" + used
        }
    }

    /// 离线那一行的说明（悬停、读屏）：最后一次回报时还开着会话，现在不知道
    static func deviceOfflineHelp(_ d: DeviceRow, now: Double) -> String {
        let seen = d.seenEpoch.map { agoPhrase(now - $0) } ?? "很久以前"
        return "\(seen)最后一次回报，当时还开着会话：可能休眠、断网，或者被强行结束了，不知道现在还用不用"
    }

    /// 「设备」标题右边的一句：“1 台在用 · 1 台闲置”；都没在用时“现在没有设备在用”
    static func devicesSummary(_ rows: [DeviceRow]) -> String {
        let inUse = rows.filter { $0.state == .inUse }.count, idle = rows.filter { $0.state == .idle }.count
        var parts: [String] = []
        if inUse > 0 { parts.append("\(inUse) 台在用") }
        if idle > 0 { parts.append("\(idle) 台闲置") }
        return parts.isEmpty ? "现在没有设备在用" : parts.joined(separator: " · ")
    }

    // MARK: API key（PROTOCOL 2.2 的 api_key）

    /// 官方账号 API key 那一行下面的一句。只有 cca 说这把 key 会经设备统计的服务器加密同步（`shared`）时才有：
    /// 从别的设备同步来的说哪台、什么时候记下的；这台 Mac 记下的说别的设备也能看到；没同步成的照 cca 给的原因说。
    static func apiKeyShare(_ k: APIKeyInfo, now: Double, calendar: Calendar = .current) -> String? {
        guard k.shared else { return nil }
        if let err = k.syncError { return "没能和其他设备同步：\(err)" }
        guard k.saved else { return "记下后，登录了这个账号的其他设备也能看到（加密同步，Team 的服务器看不到 key）。" }
        let when = k.savedEpoch.map { " " + pastTime($0, now: now, calendar: calendar) } ?? ""
        if let from = k.fromDevice { return "\(from)\(when) 记下，已加密同步到这台 Mac。" }
        return "这台 Mac\(when) 记下，登录了这个账号的其他设备也能看到（加密同步）。"
    }

    /// 「记录…」「输入…」的说明（悬停）
    static func apiKeyRecordHelp(isAPI: Bool, shared: Bool) -> String {
        if isAPI { return "输入这个账号的 key，存进这台 Mac 的钥匙串" }
        return shared
            ? "记一把 API key 备查（比如在控制台里建的）：存进这台 Mac 的钥匙串，并加密同步给登录了这个账号的其他设备；cca 不会拿它登录或启动这个账号"
            : "记一把 API key 备查（比如在控制台里建的）：只存进这台 Mac 的钥匙串，cca 不会拿它登录或启动这个账号"
    }

    /// 分模型的每周额度（Fable）下面那句话：它和本周总额度的关系（份额是 cca 认出来给的，PROTOCOL 3.1）。Fable 用的是本周总额度的一部分，
    /// 不是另外一份：平时“Fable 最多占本周额度的 50%，已占 6%”；它那份用完了时说清楚账号没用完：“Fable 的份额用完了，别的模型还能用本周
    /// 剩下的 49%”（本周的也用完了就只说前半句）。名字写在句子里：只写“最多用本周额度的 50%”会被看成整个账号最多只能用一半。
    /// cca 没认出份额时只说它的额度单独计。
    static func modelShare(_ m: ModelWindow, weekly: UsageWindow?) -> String {
        // 剩下多少按显示出来的百分比减（本周 51% 就说剩 49%，两个数加起来是 100）
        let left = weekly.flatMap { w in w.reset ? nil : w.utilization.map { 100 - $0.rounded() } }
        return modelShare(nonEmpty(m.name) ?? "这个模型", allowance: m.allowancePercentOfWeekly, used: m.usedPercentOfWeekly,
                          full: !m.window.reset && (m.window.utilization ?? 0) >= 100, weeklyLeft: left)
    }

    static func modelShare(_ name: String, allowance: Double?, used: Double?, full: Bool = false, weeklyLeft: Double? = nil) -> String {
        guard let allowance else { return "\(name) 的额度单独计" }
        if full {
            return "\(name) 的份额用完了" + (weeklyLeft.flatMap { $0 > 0 ? "，别的模型还能用本周剩下的 \(percent($0))" : nil } ?? "")
        }
        return "\(name) 最多占本周额度的 \(percent(allowance))" + (used.map { "，已占 \(percent($0))" } ?? "")
    }

    /// 什么时候过期（重置机会）：“11 月 20 日过期”（今年以内不写年份），一天以内写还有多久（“5 小时 3 分后过期”）。
    static func expiryText(epoch: Double, now: Double, calendar: Calendar = .current) -> String {
        if epoch <= now { return "已过期" }
        if epoch - now < 86400 { return span(epoch - now) + "后过期" }
        return dayText(epoch, now: now, calendar: calendar) + "过期"
    }

    /// 重置机会什么时候过期：「最早一次 11 月 21 日过期」（只有一次时不说最早）；不过期、cca 没说时是 nil。
    static func resetExpiry(_ rc: ResetCredits, now: Double, calendar: Calendar = .current) -> String? {
        guard let e = rc.nextExpiresEpoch else { return nil }
        return ((rc.availableCount ?? 0) > 1 ? "最早一次 " : "") + expiryText(epoch: e, now: now, calendar: calendar)
    }

    /// 总览卡片的提示和读屏：「还能重置额度 2 次，最早一次 11 月 21 日过期」；没有重置机会时是 nil。
    static func resetSummary(_ rc: ResetCredits?, now: Double, calendar: Calendar = .current) -> String? {
        guard let rc, let n = rc.availableCount, n > 0 else { return nil }
        let count = "还能重置额度 \(n) 次"
        return resetExpiry(rc, now: now, calendar: calendar).map { count + "，" + $0 } ?? count
    }

    /// Claude Code 登录时记下的套餐过时了（cca 给的 `outdated_login_plan`）：「Claude Code 里记的套餐还是 Pro，账号现在是 Max」，
    /// 不知道账号现在的套餐时只说前半句。账号页那句话、用量下面那一行都用它。
    static func outdatedLoginPlan(_ old: String, current: String?) -> String {
        "Claude Code 里记的套餐还是 \(old)" + (nonEmpty(current).map { "，账号现在是 \($0)" } ?? "")
    }

    // MARK: Team（PROTOCOL 4.14）

    /// 侧栏「共享与同步」下面那句（短写，免得截断）：在 Team 里时一个说名字、几个说几个，不在时说还没加入
    static func sharingSubtitle(teams: [String]) -> String {
        if teams.isEmpty { return "还没加入 Team" }
        if teams.count == 1 { return "在「\(nonEmpty(teams[0]) ?? "Team")」里" }
        return "在 \(teams.count) 个 Team 里"
    }

    /// 筛选菜单里一项的名字：全部账号、Team 的名字（不在了的写编号的开头）、不在 Team 里的
    static func teamFilter(_ f: TeamFilter, teams: [TeamEntry]) -> String {
        switch f {
        case .all: return "全部账号"
        case .team(let id): return teams.first { $0.id == id }.map { nonEmpty($0.name) ?? String(id.prefix(8)) } ?? String(id.prefix(8))
        case .outside: return "不在 Team 里的"
        }
    }

    /// Team 那一行状态：照 cca 给的 `config.teams[].state` 说（没同步成、被移除、记录没了时 cca 给了原因）
    static func teamState(_ t: TeamEntry) -> String {
        switch t.state {
        case .ok: return t.seq > 0 ? "配置已同步" : "已同步"
        case .pending: return "还没同步过配置"
        case .local: return "还没接上同步服务器"
        case .removed: return nonEmpty(t.error) ?? "这台设备被管理员移除了"
        case .lost: return nonEmpty(t.error) ?? "服务器的记录没了：等管理员把这台设备登记回去"
        default: return nonEmpty(t.error).map { "配置没同步成：\($0)" } ?? "配置没同步成"
        }
    }

    /// Team 的第二行：这台设备在里面叫什么、给了它几个账号、服务器
    static func teamSummary(_ t: TeamEntry) -> String {
        var parts: [String] = []
        if let d = nonEmpty(t.device) { parts.append("这台设备叫 \(d)") }
        parts.append(t.accounts > 0 ? "给了这台设备 \(t.accounts) 个账号" : "没有给这台设备账号")
        if let h = nonEmpty(t.host) { parts.append(h) }
        return parts.joined(separator: " · ")
    }

    /// 管理员看一台设备：状态、给了它几个账号、它手里的配置是不是最新的（不说第几版：序号只是 cca 用来比新旧的）
    static func teamDeviceDetail(_ d: TeamDevice, accounts total: Int, now: Double) -> String {
        var parts: [String] = []
        switch d.state {
        case .joined, .local:
            if let age = d.seenAge { parts.append(agoPhrase(age) + "来过") } else { parts.append(d.state == .local ? "还没接上服务器" : "加入了") }
        case .invited:
            parts.append(d.expiresEpoch.map { $0 > now ? "邀请还没用（" + untilPhrase($0 - now) + "后过期）" : "邀请过期了" } ?? "邀请还没用")
        case .expired: parts.append("邀请过期了，没用上")
        case .left: parts.append("它自己退出了")
        case .revoked: parts.append("移除了")
        default: parts.append(d.state.rawValue)
        }
        if d.state.active {
            parts.append(d.grantsAll ? "全部账号" : "\(d.grantCount(of: total)) 个账号")
            if let have = d.have, d.seq > 0 { parts.append(have >= d.seq ? "配置是最新的" : "还没同步到最新的配置") }
        }
        return parts.joined(separator: " · ")
    }

    /// 还有多久（邀请多久以后过期）：几天、几小时、几分钟
    static func untilPhrase(_ seconds: Double) -> String {
        let s = max(0, seconds)
        if s >= 2 * 86400 { return "\(Int(s / 86400)) 天" }
        if s >= 2 * 3600 { return "\(Int(s / 3600)) 小时" }
        return "\(max(1, Int(s / 60))) 分钟"
    }

    // MARK: 远程会话（PROTOCOL 9）

    /// 别的机器上的程序写的一行字（终端的标题）显示之前：控制字符、看不见的排版字符（改文字方向的、零宽的、标签字符）去掉，
    /// 换行换段当空格，截短。cca 给的会话说明（机器名、文件夹）它自己过滤过了，这个是 App 直接从终端里拿到的
    static func plainLine(_ s: String, max: Int) -> String {
        var out = String.UnicodeScalarView()
        for u in s.unicodeScalars {
            switch u.properties.generalCategory {
            case .control, .format, .surrogate, .privateUse, .unassigned: continue
            case .lineSeparator, .paragraphSeparator: out.append(" ")
            default: out.append(u)
            }
        }
        return String(String(out).trimmingCharacters(in: .whitespaces).prefix(max))
    }

    /// 一个会话的名字：「Claude · 工作」（工具不认识、没给时只有账号的名字）
    static func remoteTitle(_ s: RemoteSession) -> String {
        [s.tool.map { cliProduct($0) }, nonEmpty(s.name)].compactMap { $0 }.joined(separator: " · ")
    }

    /// 开着的终端那一排里，一个会话的名字：标题后面带上文件夹的最后一段（同一个账号开着几个会话时分得开）：「Claude Code · 工作 · api」
    static func remoteTab(_ s: RemoteSession) -> String {
        let title = nonEmpty(remoteTitle(s)) ?? s.short
        guard let last = nonEmpty(s.cwd)?.split(separator: "/").last.map(String.init), last != "~" else { return title }
        return title + " · " + last
    }

    /// 会话里跑的是哪个命令行工具（这里说的是终端里的工具，不是账号的品牌）
    static func cliProduct(_ tool: ToolID) -> String {
        switch tool {
        case .claude: return "Claude Code"
        case .codex: return "Codex"
        case .agy: return "Antigravity"
        default: return tool.rawValue
        }
    }

    /// 会话的第二行：「~/srv/api · 开了 2 小时 · 另有 1 处接着」
    static func remoteDetail(_ s: RemoteSession, now: Double) -> String {
        var parts: [String] = []
        if let d = nonEmpty(s.cwd) { parts.append(d) }
        if let t = s.sinceEpoch { parts.append("开了 " + span(now - t)) }
        if let open = s.localTerminal { parts.append(open ? "启动它的终端还开着" : "启动它的终端已经关了") }
        if s.viewers > 0 { parts.append("另有 \(s.viewers) 处接着") }
        return parts.joined(separator: " · ")
    }

    /// 接不上的会话为什么接不上；接得上的是 nil
    static func remoteStateNote(_ s: RemoteSession) -> String? {
        switch s.state {
        case .ok: return nil
        case .busy: return "没有回应"
        case .offline: return "那台机器不在线"
        default: return "暂时接不上"
        }
    }

    static func remoteAccess(_ a: RemoteAccess) -> String {
        switch a {
        case .control: return "看和操作"
        case .view: return "只看"
        case .none: return "不让接入"
        default: return a.rawValue
        }
    }

    /// 配对过的机器的第二行：「11 月 3 日配对 · 配对码是这台 Mac 显示的」
    static func remotePeerDetail(_ p: RemotePeer, now: Double, calendar: Calendar = .current) -> String {
        var parts: [String] = []
        if let t = p.pairedEpoch { parts.append(pastTime(t, now: now, calendar: calendar) + " 配对") }
        parts.append(p.web ? "手机在浏览器里接入" : p.shown ? "配对码是这台 Mac 显示的" : "配对码是它显示的")
        return parts.joined(separator: " · ")
    }

    /// 界面里的终端结束以后的那句话（`cca remote attach` 的退出码，PROTOCOL 9.3）。listed：会话还在最近一次查到的列表里
    static func remoteEnded(code: Int32, closedByUser: Bool, listed: Bool) -> String {
        if closedByUser { return "已断开，会话在那边照常运行" }
        switch code {
        case 65: return "没有这个会话了（可能已经结束）"
        case 66: return "没能经服务器接上那台机器"
        case 68: return "连接断了，没有再接上（原因在终端的最后一行）"
        case 64, 69: return "没接上（退出码 \(code)）"
        default: return listed ? "已断开，会话在那边照常运行" : (code == 0 ? "会话已经结束" : "会话已经结束（退出码 \(code)）")
        }
    }

    static func product(_ tool: ToolID) -> String {
        switch tool {
        case .claude: return "Claude"
        case .codex: return "ChatGPT"
        case .agy: return "Antigravity"
        default: return tool.rawValue
        }
    }

    static func login(_ s: LoginState, loginEmail: String?) -> String {
        switch s {
        case .loggedIn: return "已登录"
        case .loggedOut: return "还没登录"
        case .emailMismatch:
            if let e = nonEmpty(loginEmail) { return "登录的是 \(e)" }
            return "登录的不是这个账号"
        case .api: return "第三方 API"
        default: return "登录状态未知"
        }
    }

    static func canWhy(_ w: CanWhy, appLabel: String, tool: ToolID) -> String {
        switch w {
        case .apiAccount: return "第三方 API 账号没有桌面 App、网页版和订阅用量"
        case .appMissing: return "没装 \(appLabel)"
        case .noChrome: return "没装 Google Chrome：网页版要经账号的出口打开"
        case .notLoggedIn: return "还没登录，查不了用量"
        case .toolMissing: return "没装 \(cliCommand(tool)) 命令"
        case .unsupported: return tool == .agy ? "Antigravity 账号没有桌面 App 和能查的用量接口" : "这种账号没有这个功能"
        default: return "现在不可用"
        }
    }

    static func usageError(_ c: UsageErrorCode) -> String {
        switch c {
        case .login: return "没登录"
        case .mismatch: return "登录的不是这个账号"
        case .route: return "没有可用的出口"
        case .port: return "端口连不上"
        case .region: return "出口在不可用地区"
        case .trace: return "查不到出口"
        case .unsupported: return "这个版本不支持查用量"
        case .timeout: return "查询超时"
        case .nolimits: return "没有拿到用量数据"
        case .auth: return "登录已失效"
        default: return "查询失败"
        }
    }

    static func healthResult(_ r: HealthResult, region: String?) -> String {
        switch r {
        case .ok: return "出口正常"
        case .lowPurity: return "纯净度偏低"
        case .restricted: return nonEmpty(region).map { "出口在不可用地区（\($0)）" } ?? "出口在不可用地区"
        case .noExit: return "拿不到出口 IP"
        case .noScore: return "检测网站没给评分"
        case .portClosed: return "端口连不上"
        case .apiOk: return "接口连得上"
        case .apiUnreachable: return "接口连不上"
        default: return "检测结果未知"
        }
    }

    static func leak(_ s: LeakState?, what: String) -> String {
        switch s {
        case .some(.ok): return "\(what) 正常"
        case .some(.leak): return "\(what) 有泄露风险"
        case .some(.note): return "\(what) 有提示"
        case .some(.failed): return "\(what) 没测成"
        case .none: return "\(what) 没测"
        default: return "\(what) 未知"
        }
    }

    /// 本机 UDP 走节点那一行（节点分流页，PROTOCOL 7.1）：照 cca 给的状态说。
    static func localUDP(_ u: LocalUDP) -> String {
        let node = nonEmpty(u.node) ?? "—"
        switch u.state {
        case .off: return "本机 UDP：直连（浏览器的 WebRTC、QUIC 这些不经节点）"
        case .ok:
            if let other = nonEmpty(u.otherVPN) { return "本机 UDP：让给了另一个 VPN（\(other)），它关掉后自动经 \(node) 出去" }
            if u.running { return "本机 UDP：经 \(node) 出去（TCP 照旧直连）" }
            return u.paused ? "本机 UDP：在家时路由器管着，先停着（出门在外自动经 \(node) 出去）" : "本机 UDP：设了经 \(node)，系统服务没在运行"
        case .stale: return "本机 UDP：经 \(node)，装着的配置旧了，要重新装一次"
        case .missing: return "本机 UDP：设了经 \(node)，系统服务却不在了，要重新装"
        case .undefined: return "本机 UDP：内置代理里已经没有 \(node) 转发 UDP 的定义"
        default: return "本机 UDP：状态认不出（在终端里运行 cca local udp 看看）"
        }
    }

    static func slotState(_ s: SlotState) -> String {
        switch s {
        case .ok: return "正常"
        case .drift: return "选错节点"
        case .closed: return "连不上"
        case .unknown: return "未知"
        case .empty: return "空闲"
        default: return s.rawValue.isEmpty ? "未知" : s.rawValue
        }
    }

    static func route(_ r: Route?) -> String {
        switch r {
        case .some(.router): return "路由器"
        case .some(.local): return "本机 Clash"
        case .some(.direct): return "直连"
        case .some(.none): return "无"
        default: return "未知"
        }
    }

    /// 出口检测是经哪边测的：“经路由器” / “经本机 Clash” / “直连”；不知道时为 nil。
    static func via(_ r: Route?) -> String? {
        switch r {
        case .some(.router): return "经路由器"
        case .some(.local): return "经本机 Clash"
        case .some(.direct): return "直连"
        default: return nil
        }
    }

    static func routePref(_ r: RoutePref?) -> String {
        switch r {
        case .some(.auto): return "自动（连得上路由器走路由器，连不上走本机 Clash）"
        case .some(.router): return "只走路由器"
        case .some(.local): return "只走本机 Clash"
        default: return "未知"
        }
    }

    static func step(_ s: StepCode, appLabel: String) -> String {
        switch s {
        case .sync: return "同步路由器…"
        case .preflight: return "检测出口…"
        case .prepare: return "准备账号目录…"
        case .open: return "打开 \(appLabel)…"
        default: return "处理中…"
        }
    }

    /// 一个账号正在做的事（按钮旁边、工位上的进度）：有 step 事件时按它说，否则按是什么操作。
    static func activity(_ act: Activity, appLabel: String) -> String {
        if let s = act.step { return step(s, appLabel: appLabel) }
        switch act.kind {
        case .app: return "打开 \(appLabel)…"
        case .web: return "打开网页版…"
        case .billing: return "打开账单页…"
        case .limitResets: return "打开用量页…"
        case .check: return "检测出口…"
        case .sync: return "同步路由器…"
        case .rename: return "改别名…"
        case .reset: return "重置额度…"
        case .mail: return "打开登录邮件…"
        }
    }

    static func liveError(_ e: LiveIssue) -> String {
        if let m = nonEmpty(e.message) { return m }
        switch e.code {
        case .unreachable: return "连不上"
        case .statusFailed: return "读不到状态"
        case .notConfigured: return "没配置"
        case .notRunning: return "没在运行"
        default: return "出错了"
        }
    }

    /// 对账时要做的改动（`notices`）：都是照 Team 给这台设备的配置做的（cca 没说是哪个 Team，这里也不说）
    static func notice(_ n: StateNotice) -> String {
        let who = n.key ?? ""
        switch n.code {
        case .policyAdded: return "Team 的配置会加入账号 \(who)（节点 \(n.node ?? "—")）"
        case .policyAlias: return "Team 的配置会把 \(n.oldKey ?? "账号") 改名为 \(who)"
        case .policyPinned: return "Team 的配置会把 \(who) 的节点从 \(n.oldNode ?? "—") 改成 \(n.node ?? "—")"
        case .nodesFilled: return "Team 配置里的节点 \(n.node ?? "") 会加入可选节点"
        default: return "下次运行 cca 时会有改动"
        }
    }

    static func stateError(_ e: StateIssue) -> String {
        if let m = nonEmpty(e.message) { return m }
        switch e.code {
        case .configUnreadable: return "读不了 cca 的设置"
        case .tableUnreadable: return "读不了账号表"
        case .accountUnreadable: return "读不了账号的登录状态（这个账号当作没登录）"
        case .tableInvalid: return "账号表有错误"
        default: return "cca 报告了一个问题"
        }
    }

    // MARK: 办公室

    static func department(_ d: Employee.Department) -> String {
        switch d {
        case .claude: return "Claude 账号"
        case .chatgpt: return "ChatGPT 账号"
        case .antigravity: return "Antigravity 账号"
        case .contractors: return "第三方 API 账号"
        }
    }

    /// 工具自己的命令（终端里启动的那个）：claude、codex、agy
    static func cliCommand(_ tool: ToolID) -> String { tool == .claude ? "claude" : tool.rawValue }

    /// 在终端里用的那个程序的叫法（账号页「终端」按钮的提示）：Claude Code、Codex、agy
    static func cliName(_ tool: ToolID) -> String {
        switch tool {
        case .codex: return "Codex"
        case .agy: return "agy"
        default: return "Claude Code"
        }
    }

    /// 没有查用量接口的工具，用量那一节说去哪里看
    static func noUsage(_ tool: ToolID) -> String {
        tool == .agy ? "Antigravity 没有能在 agy 外面查的用量接口：在 agy 里用 /usage 看。" : "这种账号没有能查的用量接口。"
    }

    /// 套餐档位的叫法（读屏用；界面上靠徽章的样子区分）。不知道的档位为 nil。
    static func planTier(_ t: PlanTier) -> String? {
        switch t {
        case .entry: return "入门"
        case .standard: return "标准"
        case .advanced: return "进阶"
        case .premium: return "尊贵"
        default: return nil
        }
    }

    /// 读屏念的套餐：“Ultra，尊贵套餐”；不知道档位时只念名字。
    static func planName(_ plan: String, tier: PlanTier) -> String {
        planTier(tier).map { "\(plan)，\($0)套餐" } ?? plan
    }

    /// 额度用完的员工几点回来（显示器上写的）：今天的“16:00”，明天的“明天 09:00”，再远“周二 16:00”。
    /// today 写在今天的时间前面（周结算提醒写“今天 19:55”）。
    static func backTime(epoch: Double, now: Double, calendar: Calendar = .current, today: String = "") -> String {
        let d = Date(timeIntervalSince1970: epoch), n = Date(timeIntervalSince1970: now)
        if calendar.isDate(d, inSameDayAs: n) { return today + clock(epoch, calendar: calendar) }
        if let t = calendar.date(byAdding: .day, value: 1, to: n), calendar.isDate(d, inSameDayAs: t) {
            return "明天 " + clock(epoch, calendar: calendar)
        }
        return weekdayTime(epoch, calendar: calendar)
    }

    /// 周结算提醒（cca 给的 use_first）：“本周额度还剩 59%，明天 19:55 重置，建议先用这个账号”。
    /// 账号页那句话、总览卡片和侧栏 / 菜单栏沙漏的悬停说明、读屏都用这一句。
    static func useFirst(_ f: UseFirst, now: Double, calendar: Calendar = .current) -> String {
        let when = backTime(epoch: f.untilEpoch, now: now, calendar: calendar, today: "今天 ")
        return "本周额度还剩 \(f.leftPercent)%，\(when) 重置，建议先用这个账号"
    }

    // MARK: 每周用量（cca 给的 usage_weeks）

    /// 过完的几周的统计（账号页「每周用量」图下面那一行）：「记下的 8 周平均用到 63% · 2 周用完 · 3 周没用到一半」；
    /// 一周都还没过完时是 nil。
    static func weeksSummary(_ s: UsageWeeksSummary) -> String? {
        guard let avg = weeksAverage(s) else { return nil }
        return ([s.weeks == 1 ? "记下的 1 周用到 \(avg)" : "记下的 \(s.weeks) 周平均用到 \(avg)"] + weeksNotes(s)).joined(separator: " · ")
    }

    /// 平均每周用到多少：「63%」；一周都还没过完时是 nil
    static func weeksAverage(_ s: UsageWeeksSummary) -> String? {
        guard s.weeks > 0, let avg = s.averagePercent else { return nil }
        return "\(avg)%"
    }

    /// 几周用完、几周没用到一半（用量统计页平均数下面那一行）；都没有时是空的
    static func weeksNotes(_ s: UsageWeeksSummary) -> [String] {
        (s.maxed > 0 ? ["\(s.maxed) 周用完"] : []) + (s.underHalf > 0 ? ["\(s.underHalf) 周没用到一半"] : [])
    }

    /// 一周的细节（鼠标指着柱子时、读屏）：「11 月 6 日–13 日：用到 87%，中间重置过 1 次」；
    /// 还没过完的这一周：「本周到现在用了 20%，周四 10:00 重置」
    static func weekDetail(_ w: UsageWeek, now: Double, calendar: Calendar = .current) -> String {
        let resets = w.resets > 0 ? "中间重置过 \(w.resets) 次" : nil
        if w.current {
            return "本周到现在用了 \(percent(w.usedPercent))" + (resets.map { "（\($0)）" } ?? "") + "，"
                + resetText(epoch: w.endEpoch, now: now, calendar: calendar)
        }
        return weekRange(w, now: now, calendar: calendar) + "：用到 \(percent(w.usedPercent))" + (resets.map { "，" + $0 } ?? "")
            + (w.partial ? "；结束前一天没查到，实际可能更多" : "")
    }

    /// 「11 月 6 日–13 日」、「10 月 30 日–11 月 6 日」（不是今年的带年份）
    static func weekRange(_ w: UsageWeek, now: Double, calendar: Calendar = .current) -> String {
        let a = calendar.dateComponents([.year, .month], from: Date(timeIntervalSince1970: w.startEpoch))
        let b = calendar.dateComponents([.year, .month, .day], from: Date(timeIntervalSince1970: w.endEpoch))
        let start = dayText(w.startEpoch, now: now, calendar: calendar)
        if a.year == b.year && a.month == b.month { return start + "–\(b.day ?? 0) 日" }
        return start + "–" + dayText(w.endEpoch, now: now, calendar: calendar)
    }

    /// 柱子下面的日期：这一周从哪天开始（「11/6」）；还没过完的写「本周」
    static func weekLabel(_ w: UsageWeek, calendar: Calendar = .current) -> String {
        if w.current { return "本周" }
        let c = calendar.dateComponents([.month, .day], from: Date(timeIntervalSince1970: w.startEpoch))
        return "\(c.month ?? 0)/\(c.day ?? 0)"
    }

    /// 从什么时候开始记的：「从 11 月 11 日开始记」
    static func weeksSince(_ epoch: Double, now: Double, calendar: Calendar = .current) -> String {
        "从 " + dayText(epoch, now: now, calendar: calendar) + "开始记"
    }

    /// 侧栏「客户端更新」右边的标签：有几个客户端有新版本。
    static func clientUpdatesTag(_ n: Int) -> String { "\(n) 个可更新" }

    /// “16:00 恢复”；时间已经过了是“马上恢复”（等下一次读到新的用量）。
    static func backAt(epoch: Double, now: Double, calendar: Calendar = .current) -> String {
        epoch <= now ? "马上恢复" : backTime(epoch: epoch, now: now, calendar: calendar) + " 恢复"
    }

    /// 在忙什么：“Claude App 开着 · 命令行里 2 个会话，已 12 分钟”。
    static func working(app: Bool, appLabel: String, sessions: Int, since: Double?, now: Double) -> String {
        var parts: [String] = []
        if app { parts.append("\(appLabel) 开着") }
        if sessions > 0 {
            var s = sessions == 1 ? "终端里在用" : "终端里 \(sessions) 个会话"
            if let since { s += "，" + elapsed(since: since, now: now) }
            parts.append(s)
        }
        return parts.isEmpty ? "在用" : parts.joined(separator: " · ")
    }

    /// 这个账号在忙什么（头像绿点的悬停提示、工牌上的标签）；不给 now 就不写用了多久。
    static func working(_ a: Account, appLabel: String, now: Double? = nil) -> String {
        working(app: a.appRunning, appLabel: appLabel, sessions: a.cliSessions, since: now == nil ? nil : a.cliSince, now: now ?? 0)
    }

    /// “刚开始” / “已 12 分钟”
    private static func elapsed(since: Double, now: Double) -> String {
        now - since < 60 ? "刚开始" : "已 " + span(now - since)
    }

    static func ailment(_ a: Employee.Ailment) -> String {
        switch a {
        case .portClosed: return "端口连不上"
        case .wrongNode: return "节点选错了"
        case .exit(let r, let region): return healthResult(r, region: region)
        }
    }

    /// 工位下面那一行：现在怎么样。brief：工位上的短写法（在忙什么看左上角的小图标，整句在悬停提示和读屏里）。
    /// 在用时有问题，问题跟在后面：“在用 · 端口连不上”（整句：“Claude App 开着 · 端口连不上”）。
    static func mood(_ e: Employee, appLabel: String, now: Double, brief: Bool = false) -> String {
        guard e.mood == .working else { return problem(e.mood, now: now, brief: brief) ?? "空闲" }
        let doing = working(app: e.app, appLabel: appLabel, sessions: e.sessions, since: e.since, now: now)
        if let t = e.trouble, let p = problem(t, now: now, brief: brief) { return (brief ? "在用" : doing) + " · " + p }
        return brief ? "在用" + (e.since.map { "，" + elapsed(since: $0, now: now) } ?? "") : doing
    }

    /// 有问题时的那一句；在用、空闲是 nil
    private static func problem(_ m: Employee.Mood, now: Double, brief: Bool) -> String? {
        switch m {
        case .absent: return "还没登录"
        case .wrongPerson(let email): return email.map { "登录的是 \($0)" } ?? "登录的不是这个账号"
        case .expired: return brief ? "登录已失效" : "登录已失效，要重新登录"
        case .unwell(let a): return ailment(a)
        case .resting(let until): return "额度用完了" + (until.map { "，" + backAt(epoch: $0, now: now) } ?? "")
        case .working, .idle: return nil
        }
    }

    /// 头条之外的小事（工位右上角的标记），照 Diagnosis 的顺序。
    static func flags(_ fs: [Employee.Flag], appLabel: String) -> [String] {
        fs.map { f in
            switch f {
            case .nearLimit: return "额度快用完了"
            case .noApp: return "没装 \(appLabel)"
            case .lowPurity: return "出口 IP 的纯净度偏低"
            case .leak: return "DNS 或 WebRTC 可能泄露"
            case .neverChecked: return "还没检测过出口"
            }
        }
    }

    /// 总览顶上那一行：“8 个账号 · 2 个在用 · 1 个额度用完 · 1 个没登录”（0 的不写）；brief 只写几个账号、几个在用（侧栏）。
    static func officeSummary(_ c: OfficeCensus, brief: Bool = false) -> String {
        if c.total == 0 { return "还没有账号" }
        let rest: [(Int, String)] = brief ? [(c.working, "在用")]
            : [(c.working, "在用"), (c.idle, "空闲"), (c.resting, "额度用完"), (c.absent, "没登录"), (c.wrongPerson, "登错号"), (c.expired, "登录失效"), (c.unwell, "有问题")]
        return (["\(c.total) 个账号"] + rest.filter { $0.0 > 0 }.map { "\($0.0) 个\($0.1)" }).joined(separator: " · ")
    }

    /// 读屏念的一整句：“工作，Claude 账号，Ultra，尊贵套餐，Claude App 开着”。
    static func deskLabel(_ e: Employee, appLabel: String, now: Double, resets: ResetCredits? = nil) -> String {
        let a = e.account
        var parts = [a.displayName, department(e.department)]
        if let p = nonEmpty(a.plan) { parts.append(planName(p, tier: e.tier)) }
        parts.append(mood(e, appLabel: appLabel, now: now))
        parts += flags(e.flags, appLabel: appLabel)
        if let f = e.useFirst { parts.append(useFirst(f, now: now)) }
        if let r = resetSummary(resets, now: now) { parts.append(r) }
        return parts.joined(separator: "，")
    }
}

enum Attention {
    /// 菜单栏图标要不要带标记：路由器待同步、有槽位选错节点、或者有账号的用量 ≥ 90%。
    static func needed(sync: SyncState?, live: LiveDoc?, usages: [UsageEntry], now: Double) -> Bool {
        if sync == .pending { return true }
        if live?.hasDrift ?? false { return true }
        return usages.contains { u in
            [u.fiveHour, u.sevenDay].contains { w in
                guard let w, let p = w.utilization, !w.isReset(now: now), u.source != .none else { return false }
                return p >= 90
            }
        }
    }
}
