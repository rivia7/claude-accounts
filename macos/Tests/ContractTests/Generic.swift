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
    let gui = raw["gui"] as? [String: Any]
    check("\(name) gui.app 是 ~/Applications/cca.app", (gui?["app"] as? String)?.hasSuffix("/Applications/cca.app") == true,
          "\(String(describing: gui?["app"]))")
    knownEnums(name, auditHello(doc))
    return doc
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

func accountInvariants(_ name: String, _ t: ToolState, _ a: Account, _ ra: [String: Any]) {
    let n = "\(name) \(t.id.rawValue)/\(a.key)"
    check("\(n) tool 填对了", a.tool == t.id)
    check("\(n) check 总是可用", a.can.check)
    for k in a.canWhy.keys {
        let off = (k == "app" && !a.can.app) || (k == "web" && !a.can.web) || (k == "usage" && !a.can.usage)
        check("\(n) can_why.\(k) 对应一个不可用的按钮", off)
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
        if let last = t.last { check("\(name) \(t.id.rawValue) last 是表里的账号", t.accounts.contains { $0.key == last }, last) }
        for (j, a) in t.accounts.enumerated() {
            accountInvariants(name, t, a, j < rawAccounts.count ? rawAccounts[j] : [:])
        }
    }
    if let r = doc.routing {
        routingInvariants(name, r, doc, raw["routing"] as? [String: Any] ?? [:])
    } else {
        check("\(name) 有 routing", false)
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
func injectUnknown(_ x: Any, parentKey: String? = nil) -> Any {
    if var d = x as? [String: Any] {
        for (k, v) in d { d[k] = injectUnknown(v, parentKey: k) }
        if parentKey != "can_why" && parentKey != "tools" {
            d["zz_unknown_object"] = ["nested": [1, 2.5, "x", NSNull()] as [Any], "flag": true] as [String: Any]
            d["zz_unknown_text"] = "future"
        }
        return d
    }
    if let a = x as? [Any] { return a.map { injectUnknown($0, parentKey: parentKey == "tools" ? "tool" : nil) } }
    return x
}

private let enumKeys: Set<String> = ["login", "source", "error_code", "result", "state", "sync", "local_sync", "route", "route_pref",
                                     "client", "backend", "dns", "webrtc", "severity", "default", "item", "plan_tier"]

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
                                "sessions", "since_epoch"]
    let boolean: Set<String> = ["stale", "pinned", "is_last", "running", "applies", "reset", "reachable", "hook", "cron", "listening"]
    let text: Set<String> = ["plan", "plan_tier", "label", "message", "org", "trust_label", "balance", "error", "local_node", "selected"]
    if var d = x as? [String: Any] {
        for (k, v) in d {
            if numeric.contains(k) {
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
    [s.tools.count, s.routing?.slots.count ?? -1, s.routing?.nodes.count ?? -1, s.notices.count, s.errors.count]
        + s.tools.map(\.accounts.count)
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
        }
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
        case .other: break
        }
    }
    return a.unknown
}
