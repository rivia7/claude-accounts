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
            // ChatGPT App（Codex 的桌面版）没有固定的下载地址：cca 的说明里写着用 codex app 打开它的安装页
            return tool == .codex ? .none : .openURL(claudeDownload)
        default:
            return .none
        }
    }

    var title: String? {
        switch self {
        case .none: return nil
        case .routerSync: return "同步路由器"
        case .handoff(let h): return "在终端里运行 \(h.display)"
        case .openURL: return "去下载"
        }
    }
}

enum Texts {
    private static let weekdays = ["日", "一", "二", "三", "四", "五", "六"]

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
    private static func weekdayTime(_ epoch: Double, calendar: Calendar) -> String {
        let c = calendar.dateComponents([.weekday], from: Date(timeIntervalSince1970: epoch))
        return "周" + weekdays[((c.weekday ?? 1) - 1 + 7) % 7] + " " + clock(epoch, calendar: calendar)
    }

    /// “09:00”
    private static func clock(_ epoch: Double, calendar: Calendar) -> String {
        let c = calendar.dateComponents([.hour, .minute], from: Date(timeIntervalSince1970: epoch))
        return String(format: "%02d:%02d", c.hour ?? 0, c.minute ?? 0)
    }

    static func percent(_ p: Double) -> String {
        "\(Int(p.rounded()))%"
    }

    static func product(_ tool: ToolID) -> String {
        switch tool {
        case .claude: return "Claude"
        case .codex: return "ChatGPT"
        default: return tool.rawValue
        }
    }

    static func login(_ s: LoginState, loginEmail: String?) -> String {
        switch s {
        case .loggedIn: return "已登录"
        case .loggedOut: return "未登录"
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
        case .toolMissing: return tool == .codex ? "没装 codex 命令" : "没装 claude 命令"
        default: return "现在不可用"
        }
    }

    static func usageError(_ c: UsageErrorCode) -> String {
        switch c {
        case .login: return "没登录"
        case .mismatch: return "登录的不是这个账号"
        case .route: return "没有可用的出口"
        case .port: return "出口端口不通"
        case .region: return "出口在不可用地区"
        case .trace: return "查不到出口"
        case .unsupported: return "这个版本不支持查用量"
        case .timeout: return "查询超时"
        case .nolimits: return "没有拿到用量数据"
        default: return "查询失败"
        }
    }

    static func healthResult(_ r: HealthResult, region: String?) -> String {
        switch r {
        case .ok: return "出口正常"
        case .lowPurity: return "纯净度低于设定"
        case .restricted: return nonEmpty(region).map { "出口在不可用地区（\($0)）" } ?? "出口在不可用地区"
        case .noExit: return "拿不到出口 IP"
        case .noScore: return "检测服务没给评分"
        case .portClosed: return "端口不通"
        case .apiOk: return "接口连得上"
        case .apiUnreachable: return "接口连不上"
        default: return "检测结果未知"
        }
    }

    static func leak(_ s: LeakState?, what: String) -> String {
        switch s {
        case .some(.ok): return "\(what) 无泄露"
        case .some(.leak): return "\(what) 泄露"
        case .some(.failed): return "\(what) 没测成"
        case .none: return "\(what) 没测"
        default: return "\(what) 未知"
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
        case .check: return "检测出口…"
        case .sync: return "同步路由器…"
        case .rename: return "改名…"
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

    static func notice(_ n: StateNotice) -> String {
        let who = n.key ?? ""
        switch n.code {
        case .policyAdded: return "维护者的映射会加入账号 \(who)（节点 \(n.node ?? "—")）"
        case .policyAlias: return "维护者的映射会把 \(n.oldKey ?? "账号") 改名为 \(who)"
        case .policyPinned: return "维护者的映射会把 \(who) 的节点从 \(n.oldNode ?? "—") 改成 \(n.node ?? "—")"
        case .nodesFilled: return "维护者的参考节点 \(n.node ?? "") 会加入可选节点"
        default: return "下次运行 cca 时会有改动"
        }
    }

    static func stateError(_ e: StateIssue) -> String {
        if let m = nonEmpty(e.message) { return m }
        switch e.code {
        case .configUnreadable: return "读不了 cca 的设置"
        case .tableUnreadable: return "读不了账号表"
        case .accountUnreadable: return "读不了账号的登录状态（这个账号当作没登录）"
        case .tableInvalid: return "账号表校验不过"
        default: return "cca 报告了一个问题"
        }
    }

    // MARK: 办公室

    static func department(_ d: Employee.Department) -> String {
        switch d {
        case .claude: return "Claude 组"
        case .chatgpt: return "ChatGPT 组"
        case .contractors: return "外包"
        }
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
    static func backTime(epoch: Double, now: Double, calendar: Calendar = .current) -> String {
        let d = Date(timeIntervalSince1970: epoch), n = Date(timeIntervalSince1970: now)
        if calendar.isDate(d, inSameDayAs: n) { return clock(epoch, calendar: calendar) }
        if let t = calendar.date(byAdding: .day, value: 1, to: n), calendar.isDate(d, inSameDayAs: t) {
            return "明天 " + clock(epoch, calendar: calendar)
        }
        return weekdayTime(epoch, calendar: calendar)
    }

    /// “16:00 回来”；时间已经过了是“马上回来”（等下一次读到新的用量）。
    static func backAt(epoch: Double, now: Double, calendar: Calendar = .current) -> String {
        epoch <= now ? "马上回来" : backTime(epoch: epoch, now: now, calendar: calendar) + " 回来"
    }

    /// 在忙什么：“Claude App 开着 · 命令行里 2 个会话，已 12 分钟”。
    static func working(app: Bool, appLabel: String, sessions: Int, since: Double?, now: Double) -> String {
        var parts: [String] = []
        if app { parts.append("\(appLabel) 开着") }
        if sessions > 0 {
            var s = sessions == 1 ? "命令行里在用" : "命令行里 \(sessions) 个会话"
            if let since { s += "，" + elapsed(since: since, now: now) }
            parts.append(s)
        }
        return parts.isEmpty ? "在忙" : parts.joined(separator: " · ")
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
    static func mood(_ e: Employee, appLabel: String, now: Double, brief: Bool = false) -> String {
        switch e.mood {
        case .absent: return "还没登录"
        case .wrongPerson(let email): return email.map { "登录的是 \($0)" } ?? "登录的不是这个账号"
        case .unwell(let a): return ailment(a)
        case .resting(let until): return "额度用完了" + (until.map { "，" + backAt(epoch: $0, now: now) } ?? "")
        case .working where brief: return "在忙" + (e.since.map { "，" + elapsed(since: $0, now: now) } ?? "")
        case .working: return working(app: e.app, appLabel: appLabel, sessions: e.sessions, since: e.since, now: now)
        case .idle: return "空闲"
        }
    }

    /// 头条之外的小事（工位右上角的标记），照 Diagnosis 的顺序。
    static func flags(_ fs: [Employee.Flag], appLabel: String) -> [String] {
        fs.map { f in
            switch f {
            case .nearLimit: return "额度快用完了"
            case .noApp: return "没装 \(appLabel)"
            case .lowPurity: return "出口 IP 的纯净度低于设定"
            case .leak: return "检测到 DNS 或 WebRTC 泄露"
            case .neverChecked: return "还没检测过出口"
            }
        }
    }

    /// 办公室顶上那一行：“8 位员工 · 2 人在忙 · 1 人休息 · 1 人没登录”（0 的不写）；brief 只写几位员工、几人在忙（侧栏）。
    static func officeSummary(_ c: OfficeCensus, brief: Bool = false) -> String {
        if c.total == 0 { return "还没有员工" }
        let rest: [(Int, String)] = brief ? [(c.working, "在忙")]
            : [(c.working, "在忙"), (c.idle, "空闲"), (c.resting, "休息"), (c.absent, "没登录"), (c.wrongPerson, "登错号"), (c.unwell, "有问题")]
        return (["\(c.total) 位员工"] + rest.filter { $0.0 > 0 }.map { "\($0.0) 人\($0.1)" }).joined(separator: " · ")
    }

    /// 读屏念的一整句：“工作，Claude 组，Ultra，尊贵套餐，Claude App 开着”。
    static func deskLabel(_ e: Employee, appLabel: String, now: Double) -> String {
        let a = e.account
        var parts = [a.displayName, department(e.department)]
        if let p = nonEmpty(a.plan) { parts.append(planName(p, tier: e.tier)) }
        parts.append(mood(e, appLabel: appLabel, now: now))
        parts += flags(e.flags, appLabel: appLabel)
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

    /// 要不要刷新用量：能查用量的账号里，有没有缓存、数据过期（stale）、或者超过 maxAge 秒没更新的。
    static func usageNeedsRefresh(_ items: [(account: Account, usage: UsageEntry?)], now: Double, maxAge: Double = 300) -> Bool {
        items.contains { item in
            guard item.account.can.usage, item.account.login == .loggedIn else { return false }
            guard let u = item.usage, let at = u.fetchedEpoch else { return true }
            return u.stale || now - at > maxAge
        }
    }
}
