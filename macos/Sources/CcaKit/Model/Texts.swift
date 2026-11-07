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
        let c = calendar.dateComponents([.weekday, .hour, .minute], from: Date(timeIntervalSince1970: epoch))
        let wd = weekdays[((c.weekday ?? 1) - 1 + 7) % 7]
        return String(format: "周%@ %02d:%02d 重置", wd, c.hour ?? 0, c.minute ?? 0)
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
