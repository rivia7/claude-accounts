import CcaRemoteKit
import Foundation

/// 演示模式（App 审核、没有电脑时自己看看）：本地模拟两台电脑、它们上面的账号和几个会话，终端里放一段预先写好的输出，敲的字只在
/// 这台手机上回显；「新建会话」挑的模板一下就“起来”，接着列在会话里。不联网、不碰钥匙串；退出演示时全丢掉
@MainActor
final class DemoBackend: Backend {
    private var all: [Peer]
    private let started = Date().timeIntervalSince1970
    /// 「新建会话」起的会话：在哪台电脑上、开场白（接入时那一屏照它放）
    private var launched: [(pid: String, session: RemoteSession, line: String?)] = []
    /// 在终端页点了「结束会话」的：不再列出来
    private var ended: Set<String> = []

    init() {
        let server = ServerRef(url: "https://demo.example.com", pins: [])!
        let now = Date().timeIntervalSince1970
        all = [
            Peer(pid: String(repeating: "a1", count: 16), fp: "SHA256:demo-studio", name: "工作室的 Mac mini", at: now - 86400 * 9, server: server,
                 team: "示例 Team"),
            Peer(pid: String(repeating: "b2", count: 16), fp: "SHA256:demo-laptop", name: "MacBook Air", at: now - 86400 * 2, server: server,
                 team: "示例 Team"),
        ]
    }

    var isDemo: Bool { true }
    func peers() -> [Peer] { all }

    func list() async -> [PeerListing] {
        try? await Task.sleep(for: .milliseconds(350))
        let t = started
        return all.map { p in
            let sessions: [RemoteSession]
            switch p.pid.prefix(2) {
            case "a1":
                sessions = [
                    // 照模板远程启动的（这台手机结束得了）
                    RemoteSession(sid: String(repeating: "1", count: 32),
                                  info: SessionInfo(machine: p.name, tool: "claude", name: "工作", cwd: "~/srv/api", since: t - 2 * 3600 - 640, viewOnly: false,
                                                    aid: DemoAccounts.aid(1), canEnd: true),
                                  online: true, viewers: 0),
                    RemoteSession(sid: String(repeating: "2", count: 32),
                                  info: SessionInfo(machine: p.name, tool: "codex", name: "个人", cwd: "~/notes", since: t - 25 * 60, viewOnly: true,
                                                    aid: DemoAccounts.aid(2)),
                                  online: true, viewers: 1),
                ]
            case "b2":
                sessions = [
                    RemoteSession(sid: String(repeating: "3", count: 32),
                                  info: SessionInfo(machine: p.name, tool: "claude", name: "账号 2", cwd: "~/Projects/site", since: t - 86400 - 3600, viewOnly: false,
                                                    aid: DemoAccounts.aid(4)),
                                  online: false, viewers: 0),
                ]
            default:
                sessions = []
            }
            let listed = sessions + launched.filter { $0.pid == p.pid }.map(\.session)
            return PeerListing(peer: p, gone: false, sessions: listed.filter { !ended.contains($0.sid) }, failure: nil,
                               status: DemoAccounts.status(p.pid, machine: p.name, now: Date().timeIntervalSince1970), statusServed: true,
                               launcher: DemoLaunch.launcher(p))
        }
    }

    func launch(peer: Peer, launcher: HostLauncher, template: LaunchTemplate, account: LaunchAccount?, cols: Int, rows: Int,
                line: String?) async -> LaunchResult {
        try? await Task.sleep(for: .milliseconds(900))
        guard launcher.online else { return .failed(RemoteFailure("nohost")) }
        let sid = (0..<16).map { _ in String(format: "%02x", UInt8.random(in: 0...255)) }.joined()
        let info = SessionInfo(machine: peer.name, tool: template.tool, name: account?.name ?? template.account, cwd: template.dir,
                               since: Date().timeIntervalSince1970, viewOnly: false, aid: account?.aid ?? template.aid, canEnd: true)
        launched.append((peer.pid, RemoteSession(sid: sid, info: info, online: true, viewers: 0), line))
        return .launched(sid: sid)
    }

    func pair(_ invite: PairInvite, progress: @escaping @Sendable (PairStep) -> Void) async throws -> Peer {
        progress(.deriving)
        try await Task.sleep(for: .milliseconds(600))
        progress(.asking)
        try await Task.sleep(for: .milliseconds(500))
        progress(.confirming)
        try await Task.sleep(for: .milliseconds(400))
        let p = Peer(pid: Bytes((0..<16).map { _ in UInt8.random(in: 0...255) }).map { String(format: "%02x", $0) }.joined(),
                     fp: "SHA256:demo", name: Texts.nonEmpty(invite.machine) ?? "演示的电脑", at: Date().timeIntervalSince1970,
                     server: invite.server, team: invite.team)
        all.append(p)
        return p
    }

    func unpair(_ peer: Peer) async -> Bool {
        all.removeAll { $0.pid == peer.pid }
        return true
    }

    func link(peer: Peer, session: RemoteSession, cols: Int, rows: Int, readOnly: Bool, fresh: Bool) -> TerminalLink {
        let mine = launched.first { $0.session.sid == session.sid }
        let l = DemoLink(session: session, readOnly: readOnly || session.info.viewOnly, launched: mine != nil, line: mine?.line)
        let sid = session.sid
        l.ended = { [weak self] in self?.ended.insert(sid) }
        return l
    }

    func fingerprint(on server: ServerRef) -> String? { "SHA256:demo" }

    // 演示模式不登记推送、不碰钥匙串
    func servers() -> [ServerRef] { [] }
    func pushSet(on server: ServerRef, token: String, env: String, sids: [String]?, usage: Bool?) async -> PushSetResult {
        .failed(RemoteFailure("demo"))
    }
    func syncNoteKeys() {}
}

/// 演示的「新建会话」：工作室的 Mac mini 上写好了三个模板（两个能带开场白），起的时候挑账号（Claude Code 的有工作、备用、API 三个，
/// Codex 的只有个人：替人选好），MacBook Air 的启动器不在线
enum DemoLaunch {
    static func launcher(_ p: Peer) -> HostLauncher? {
        switch p.pid.prefix(2) {
        case "a1":
            let t = [
                LaunchTemplate(id: "0a1b2c3d", digest: "0123456789abcdef", name: "修 api 的 bug", tool: "claude", account: "启动时选账号",
                               aid: "", dir: "~/srv/api", args: "--model opus", line: true),
                LaunchTemplate(id: "1b2c3d4e", digest: "123456789abcdef0", name: "整理笔记", tool: "codex", account: "个人",
                               aid: DemoAccounts.aid(2), dir: "~/notes", args: "", line: true),
                LaunchTemplate(id: "2c3d4e5f", digest: "23456789abcdef01", name: "看看网站", tool: "claude", account: "启动时选账号",
                               aid: "", dir: "~/Projects/site", args: "--permission-mode plan", line: false),
            ]
            let a = [
                LaunchAccount(id: DemoAccounts.id(1), tool: "claude", name: "工作", aid: DemoAccounts.aid(1)),
                LaunchAccount(id: DemoAccounts.id(3), tool: "claude", name: "备用", aid: DemoAccounts.aid(3)),
                LaunchAccount(id: DemoAccounts.id(7), tool: "claude", name: "API"),
                LaunchAccount(id: DemoAccounts.id(2), tool: "codex", name: "个人", aid: DemoAccounts.aid(2)),
            ]
            return HostLauncher(sid: String(repeating: "f1", count: 16), info: LauncherInfo(machine: p.name, templates: t, accounts: a),
                                online: true)
        case "b2":
            let t = [
                LaunchTemplate(id: "3d4e5f60", digest: "3456789abcdef012", name: "写周报", tool: "claude", account: "账号 2",
                               aid: DemoAccounts.aid(4), dir: "~/Documents", args: "", line: true),
            ]
            let a = [LaunchAccount(id: DemoAccounts.id(4), tool: "claude", name: "账号 2", aid: DemoAccounts.aid(4))]
            return HostLauncher(sid: String(repeating: "f2", count: 16), info: LauncherInfo(machine: p.name, templates: t, accounts: a),
                                online: false)
        default:
            return nil
        }
    }
}

/// 演示的账号（总览、用量统计、折合金额、共享与同步）：两台电脑上的几个账号，用量、重置时间、折合金额照现在算。在用的、额度用完了的、
/// 建议先用的、登录失效的、Antigravity 的几组额度、Codex 还能重置额度、第三方 API 的，各有一个；四个档位的套餐都有（按套餐筛：Claude 有
/// Pro、Max、Ultra），ChatGPT 的两台电脑上都有。两台电脑在同一个 Team 里（工作室的 Mac mini 是建 Team 的管理员，MacBook Air 的配置
/// 还没同步）；用过两台设备的账号列出各台设备（有一台很久没同步上来）
enum DemoAccounts {
    /// 第 n 个账号的编号、会话说明里的账号摘要
    static func id(_ n: Int) -> String { String(repeating: String(format: "%x", n), count: 16) }
    static func aid(_ n: Int) -> String { String(repeating: String(format: "c%x", n), count: 16) }

    static func status(_ pid: String, machine: String, now: Double) -> HostStatus? {
        let accounts: [[String: Any]]
        switch pid.prefix(2) {
        case "a1": accounts = [work(now), personal(now), spare(now), api(now)]
        case "b2": accounts = [second(now), go(now), gemini(now)]
        default: return nil
        }
        // 和真的一样经 JSON 读一遍（数、true / false 的样子和服务器给的一样）
        var doc: [String: Any] = ["v": 1, "machine": machine, "accounts": accounts, "at": now - 180]
        let host = "team.example.com:18445"
        switch pid.prefix(2) {
        case "a1":
            doc["cost"] = ["prices_date": "2026-11-01", "updated": (now - 240).rounded(), "unattributed": 312]
            doc["team"] = ["name": "示例 Team", "role": "admin", "state": "ok", "seq": 12, "accounts": 3, "device": "studio-mac", "host": host,
                           "joined": (now - 40 * 86400).rounded()]
        default:
            doc["cost"] = ["prices_date": "2026-11-01", "updated": (now - 3600).rounded()]
            doc["team"] = ["name": "示例 Team", "role": "member", "state": "pending", "seq": 11, "accounts": 3, "device": "macbook-air",
                           "host": host, "joined": (now - 9 * 86400).rounded()]
        }
        guard let d = try? JSONSerialization.data(withJSONObject: doc),
              let j = (try? JSONSerialization.jsonObject(with: d)) as? [String: Any] else { return nil }
        return HostStatus(json: j, age: 175, received: now)
    }

    private static func window(_ used: Double, in seconds: Double, _ now: Double) -> [String: Any] {
        ["utilization": used, "resets_epoch": (now + seconds).rounded(), "reset": false]
    }

    private static func usage(_ now: Double, _ extra: [String: Any]) -> [String: Any] {
        ["source": "live", "fetched_epoch": now - 200, "stale": false].merging(extra) { $1 }
    }

    /// 最近几周的用量：最后一周是还没过完的这一周（到 end 为止）
    private static func weeks(_ used: [Double], current: Double, end: Double, partial: Set<Int> = [], resets: [Int: Int] = [:]) -> [String: Any] {
        let week = 7 * 86400.0
        var out: [[String: Any]] = []
        for (i, p) in used.enumerated() {
            let e = end - Double(used.count - i) * week
            out.append(["start_epoch": e - week, "end_epoch": e, "used_percent": p, "resets": resets[i] ?? 0, "current": false,
                        "partial": partial.contains(i)])
        }
        out.append(["start_epoch": end - week, "end_epoch": end, "used_percent": current, "resets": 0, "current": true, "partial": false])
        let avg = used.isEmpty ? 0 : Int((used.reduce(0, +) / Double(used.count)).rounded())
        return ["since_epoch": end - Double(used.count + 1) * week + 2 * 86400, "weeks": out,
                "summary": ["weeks": used.count, "average_percent": avg, "maxed": used.filter { $0 >= 100 }.count,
                            "under_half": used.filter { $0 < 50 }.count]]
    }

    /// 在用（Claude App 开着、终端里两个会话），Ultra：5 小时 42%，本周 63%（Fable 另外计），记了 11 周；折合金额用了两台设备，
    /// 有一个新模型暂按同系列的价格估算
    private static func work(_ now: Double) -> [String: Any] {
        let weekly = now + 3 * 86400 + 4 * 3600
        return ["id": id(1), "aid": aid(1), "tool": "claude", "name": "工作", "kind": "oauth", "login": "logged_in", "plan": "Ultra",
                "tier": "premium", "busy": true, "app": true, "cli": 2, "since": (now - 2 * 3600 - 640).rounded(), "color": "#8E4EC6", "badge": "工",
                "cost": cost(160, weeks: 11, weekEnd: weekly, now: now, seed: 0.3, hit: [0.9741, 0.9752],
                             models: [("claude-opus-5-5", 0.66, nil), ("claude-sonnet-5-5", 0.30, nil), ("claude-haiku-5-6", 0.04, "claude-haiku-5-5")],
                             devices: [("studio-mac", true, 0.82, false, now - 240), ("lab-studio", false, 0.18, false, now - 1800)]),
                "usage": usage(now, ["five_hour": window(42, in: 2 * 3600 + 13 * 60, now), "seven_day": window(63, in: weekly - now, now),
                                     "model_scoped": [window(18, in: weekly - now, now).merging(["name": "Fable", "allowance_percent_of_weekly": 50,
                                                                                                 "used_percent_of_weekly": 9]) { $1 }]]),
                "weeks": weeks([58, 74, 112, 81, 66, 47, 95, 70, 39, 88, 77], current: 63, end: weekly.rounded(), partial: [8], resets: [2: 1])]
    }

    /// Codex，5 小时的额度用完了；还能重置额度 2 次
    private static func personal(_ now: Double) -> [String: Any] {
        let weekly = now + 4 * 86400 + 7 * 3600
        return ["id": id(2), "aid": aid(2), "tool": "codex", "name": "个人", "kind": "oauth", "login": "logged_in", "plan": "Plus",
                "tier": "standard", "busy": false, "color": "#30A46C", "badge": "个",
                "cost": cost(22, weeks: 6, weekEnd: weekly, now: now, seed: 1.1, hit: [0.9093, 0.9107],
                             models: [("gpt-6.2-astra", 0.74, nil), ("gpt-6.1-sol", 0.26, nil)]),
                "usage": usage(now, ["five_hour": window(100, in: 65 * 60, now), "seven_day": window(71, in: weekly - now, now),
                                     "reset_credits": ["availableCount": 2, "next_expires_epoch": (now + 9 * 86400).rounded()]]),
                "weeks": weeks([45, 62, 80, 57, 91], current: 71, end: weekly.rounded())]
    }

    /// 空闲，本周快重置了、还剩不少：建议先用
    private static func spare(_ now: Double) -> [String: Any] {
        let weekly = now + 20 * 3600
        return ["id": id(3), "aid": aid(3), "tool": "claude", "name": "备用", "kind": "oauth", "login": "logged_in", "plan": "Pro",
                "tier": "standard", "busy": false, "color": "#0A84FF", "badge": "备",
                "cost": cost(7.5, weeks: 3, weekEnd: weekly, now: now, seed: 2.0, hit: [0.968, 0.9702],
                             models: [("claude-sonnet-5-5", 0.81, nil), ("claude-opus-5-5", 0.19, nil)]),
                "usage": usage(now, ["five_hour": window(8, in: 4 * 3600 + 20 * 60, now), "seven_day": window(41, in: weekly - now, now),
                                     "use_first": ["left_percent": 59, "from_epoch": now - 3600, "until_epoch": weekly.rounded()]]),
                "weeks": weeks([33, 52], current: 41, end: weekly.rounded())]
    }

    /// 登录失效了（用的是以前查到的）
    private static func second(_ now: Double) -> [String: Any] {
        let weekly = now + 2 * 86400
        return ["id": id(4), "aid": aid(4), "tool": "claude", "name": "账号 2", "kind": "oauth", "login": "logged_in", "plan": "Max",
                "tier": "advanced", "busy": false, "color": "#E5484D", "badge": "2",
                "cost": cost(48, weeks: 4, weekEnd: weekly, now: now, seed: 2.7, hit: [0.9712, 0.9718],
                             models: [("claude-opus-5-5", 0.58, nil), ("claude-sonnet-5-5", 0.42, nil)],
                             devices: [("macbook-air", true, 0.64, false, now - 600), ("office-imac", false, 0.36, true, now - 3 * 86400 - 5 * 3600)]),
                "usage": ["source": "cache", "fetched_epoch": now - 5 * 3600, "stale": true, "error_code": "auth",
                          "five_hour": window(12, in: -3600, now), "seven_day": window(35, in: weekly - now, now)],
                "weeks": weeks([60, 44, 71], current: 35, end: weekly.rounded())]
    }

    /// 折合金额（美元换成美分，和 cca 放上来的一样）：按周 13 项（最后一项是这一周，到 weekEnd 为止）、按天 30 项（最后一项是今天）。
    /// base 是工作日平均每天多少美元（周末少一些），有记录的是最近 n 周（头一周只记了后几天：颜色浅的那一项）；合计、平均照 cca 的算法
    /// （平均算过完的、开始有记录以后的那几项）。models、devices 是各占多少（模型、占多少、暂按哪个模型的价格；设备、是不是这台、占多少、
    /// 很久没同步上来、上次同步上来的时间）
    private static func cost(_ base: Double, weeks n: Int, weekEnd: Double, now: Double, seed: Double, hit: [Double],
                             models: [(String?, Double, String?)] = [],
                             devices: [(String, Bool, Double, Bool, Double?)] = []) -> [String: Any] {
        let week = 7 * 86400.0
        let cal = Calendar.current
        let today = cal.startOfDay(for: Date(timeIntervalSince1970: now))
        let dayEnd = (cal.date(byAdding: .day, value: 1, to: today) ?? today.addingTimeInterval(86400)).timeIntervalSince1970
        let since = weekEnd - Double(n) * week + 2 * 86400
        func cents(_ usd: Double) -> Int { Int((usd * 100).rounded()) }
        let weekStarts = (0..<13).map { weekEnd - Double(13 - $0) * week }
        let weeks: [Int] = weekStarts.enumerated().map { i, start in
            guard start + week > since else { return 0 }
            var usd = base * 5.9 * (1 + 0.18 * sin(Double(i) * 1.9 + seed))
            if start < since { usd *= (start + week - since) / week }
            if i == 12 { usd *= min(max((now - start) / week, 0.05), 1) }
            return cents(usd)
        }
        let dayList = (0..<30).map { cal.date(byAdding: .day, value: $0 - 29, to: today) ?? today }
        let dayStarts = dayList.map(\.timeIntervalSince1970)
        let days: [Int] = dayList.enumerated().map { i, day in
            let start = day.timeIntervalSince1970
            guard start + 86400 > since else { return 0 }
            let wd = cal.component(.weekday, from: day)
            var usd = base * (1 + 0.25 * sin(Double(i) * 1.3 + seed)) * (wd == 1 || wd == 7 ? 0.45 : 1)
            if start < since { usd *= max(start + 86400 - since, 0) / 86400 }
            if i == 29 { usd *= min(max((now - start) / 86400, 0.15), 1) }
            return cents(usd)
        }
        func average(_ v: [Int], _ starts: [Double]) -> (Any, Int) {
            let done = v.indices.dropLast().filter { starts[$0] >= since }
            guard !done.isEmpty else { return (NSNull(), 0) }
            return (Int((Double(done.map { v[$0] }.reduce(0, +)) / Double(done.count)).rounded()), done.count)
        }
        let (wAvg, wN) = average(weeks, weekStarts), (dAvg, dN) = average(days, dayStarts)
        let wSum = weeks.reduce(0, +), dSum = days.reduce(0, +)
        var out: [String: Any] = [
            "since": since.rounded(), "weeks": weeks, "week_end": weekEnd.rounded(), "week_hours": Array(repeating: 168, count: 13),
            "days": days, "day_end": dayEnd.rounded(), "sum": [wSum, dSum], "average": [wAvg, dAvg], "average_n": [wN, dN], "cache_hit": hit,
        ]
        if !models.isEmpty {
            out["models"] = models.map { m -> [Any] in
                let (name, share, base) = m
                return [name ?? NSNull(), Int((Double(wSum) * share).rounded()), Int((Double(dSum) * share).rounded())] + (base.map { [$0] } ?? [])
            }
        }
        if !devices.isEmpty {
            out["devices"] = devices.map { d -> [Any] in
                let (name, me, share, stale, synced) = d
                return [name, me ? 1 : 0, Int((Double(wSum) * share).rounded()), Int((Double(dSum) * share).rounded()), stale ? 1 : 0,
                        synced.map { $0.rounded() } ?? NSNull()]
            }
        }
        return out
    }

    /// 按日历切的周（周一 0 点开始）这一周的结束：Antigravity 没有本周的额度，折合金额按日历的周算
    private static func calendarWeekEnd(_ now: Double) -> Double {
        var cal = Calendar.current
        cal.firstWeekday = 2
        let d = Date(timeIntervalSince1970: now)
        return (cal.dateInterval(of: .weekOfYear, for: d)?.end ?? d.addingTimeInterval(7 * 86400)).timeIntervalSince1970
    }

    /// 第三方 API 账号：没有套餐、订阅的用量（头像是钥匙）
    private static func api(_ now: Double) -> [String: Any] {
        ["id": id(7), "tool": "claude", "name": "API", "kind": "api", "login": "api", "plan": "", "tier": "", "busy": false]
    }

    /// ChatGPT 的 Go 套餐（entry），空闲
    private static func go(_ now: Double) -> [String: Any] {
        let weekly = now + 6 * 86400 + 2 * 3600
        return ["id": id(6), "aid": aid(6), "tool": "codex", "name": "副号", "kind": "oauth", "login": "logged_in", "plan": "Go",
                "tier": "entry", "busy": false, "color": "#12A594", "badge": "副",
                "cost": cost(2.4, weeks: 3, weekEnd: weekly, now: now, seed: 3.5, hit: [0.9001, 0.9023], models: [("gpt-6.1-sol", 1, nil)]),
                "usage": usage(now, ["five_hour": window(23, in: 3 * 3600 + 40 * 60, now), "seven_day": window(18, in: weekly - now, now)]),
                "weeks": weeks([26, 41], current: 18, end: weekly.rounded())]
    }

    /// Antigravity：两组额度
    private static func gemini(_ now: Double) -> [String: Any] {
        func bucket(_ used: Double, _ seconds: Double, _ slot: String) -> [String: Any] {
            window(used, in: seconds, now).merging(["slot": slot]) { $1 }
        }
        return ["id": id(5), "tool": "agy", "name": "Gemini", "kind": "oauth", "login": "logged_in", "plan": "Google AI Pro",
                "tier": "standard", "busy": false, "color": "#F76B15", "badge": "G",
                "cost": cost(9, weeks: 6, weekEnd: calendarWeekEnd(now), now: now, seed: 4.2, hit: [0.95, 0.9521],
                             models: [("gemini-3-pro", 0.78, nil), ("gemini-3-flash", 0.22, nil)]),
                "usage": usage(now, ["quota_groups": [
                    ["name": "Gemini models", "buckets": [bucket(35, 3 * 3600, "five_hour"), bucket(52, 5 * 86400, "seven_day")]],
                    ["name": "Claude and GPT models", "buckets": [bucket(86, 90 * 60, "five_hour"), bucket(64, 5 * 86400, "seven_day")]],
                ]])]
    }
}

/// 演示的终端：一段预先写好的输出（一点一点放出来），之后是一个提示符，敲的字回显，回车后答一句“这是演示”。「新建会话」起的
/// 会话是刚起来的样子（有开场白的先放它）
@MainActor
final class DemoLink: TerminalLink {
    var onOutput: (Bytes) -> Void = { _ in }
    var onSize: (Int, Int) -> Void = { _, _ in }
    var onState: (Attachment.State) -> Void = { _ in }
    var onExit: (Int32) -> Void = { _ in }
    var onNote: (Note) -> Void = { _ in }
    private let session: RemoteSession
    private let readOnly: Bool
    private let launched: Bool
    private let opening: String?
    private var line = ""
    private var closed = false
    private var opened = false
    /// 结束了（点了「结束会话」）：演示的那一头不再列它
    var ended: () -> Void = {}

    init(session: RemoteSession, readOnly: Bool, launched: Bool = false, line: String? = nil) {
        self.session = session
        self.readOnly = readOnly
        self.launched = launched
        opening = line
    }

    func start() {
        onState(.connecting)
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(500))
            guard !closed else { return }
            if !session.online {
                onState(.closed("nohost"))
                return
            }
            opened = true
            onState(.open)
            for chunk in launched ? Self.fresh(session, line: opening) : Self.script(session) {
                try? await Task.sleep(for: .milliseconds(chunk.delay))
                guard !closed else { return }
                say(chunk.text)
            }
        }
    }

    private func say(_ s: String) {
        onOutput(Bytes(s.utf8))
    }

    private static let prompt = "\r\n\u{1b}[38;5;141m›\u{1b}[0m "

    private static func script(_ s: RemoteSession) -> [(delay: Int, text: String)] {
        let dim = "\u{1b}[2m", off = "\u{1b}[0m", green = "\u{1b}[32m", violet = "\u{1b}[38;5;141m", bold = "\u{1b}[1m"
        let tool = Texts.cliProduct(s.info.tool) ?? "会话"
        return [
            (80, "\u{1b}[2J\u{1b}[H\(dim)演示：这是示意画面，不是真实的会话。输入的内容只在这台手机上显示，不会发送到任何地方。\(off)\r\n\r\n"),
            (250, "\(violet)\(bold)● \(tool)\(off)  \(dim)\(s.info.cwd)\(off)\r\n\r\n"),
            (300, "\(violet)›\(off) 把 api 的单元测试跑一遍，挂了的修好\r\n\r\n"),
            (500, "  先跑一遍看看现在的情况。\r\n\r\n"),
            (350, "  \(green)⏺\(off) \(bold)Bash\(off)(npm test)\r\n"),
            (600, "    \(dim)⎿  Tests: 2 failed, 46 passed, 48 total\(off)\r\n\r\n"),
            (500, "  两个挂的都在 \(bold)src/date.ts\(off)：时区换算少算了夏令时。改一下：\r\n\r\n"),
            (400, "  \(green)⏺\(off) \(bold)Update\(off)(src/date.ts)\r\n"),
            (300, "    \(dim)⎿  改了 1 处，加了 3 行、删了 1 行\(off)\r\n\r\n"),
            (500, "  \(green)⏺\(off) \(bold)Bash\(off)(npm test)\r\n"),
            (700, "    \(dim)⎿  Tests: 48 passed, 48 total\(off)\r\n\r\n"),
            (300, "  全部通过了。\r\n"),
            (200, s.info.viewOnly ? "\r\n\(dim)（只看：电脑只允许查看这个会话）\(off)\r\n" : prompt),
        ]
    }

    /// 「新建会话」刚起的会话：工具、文件夹，有开场白的先放它，然后是提示符
    private static func fresh(_ s: RemoteSession, line: String?) -> [(delay: Int, text: String)] {
        let dim = "\u{1b}[2m", off = "\u{1b}[0m", violet = "\u{1b}[38;5;141m", bold = "\u{1b}[1m"
        let tool = Texts.cliProduct(s.info.tool) ?? "会话"
        var out: [(delay: Int, text: String)] = [
            (80, "\u{1b}[2J\u{1b}[H\(dim)演示：这是示意画面，不是真实的会话。输入的内容只在这台手机上显示，不会发送到任何地方。\(off)\r\n\r\n"),
            (400, "\(violet)\(bold)● \(tool)\(off)  \(dim)\(s.info.cwd)\(off)\r\n\r\n"),
        ]
        if let line {
            out.append((300, "\(violet)›\(off) \(line)\r\n\r\n"))
            out.append((600, "  \(dim)（演示）在真实的会话中，开场白是会话启动后收到的第一句话，\(tool) 会接着开始处理。\(off)\r\n"))
        }
        out.append((200, prompt))
        return out
    }

    func nudge() {}

    func input(_ data: Bytes) -> Bool {
        guard opened, !closed, !readOnly else { return false }
        for b in String(decoding: data, as: UTF8.self) {
            switch b {
            case "\r", "\n":
                let said = line.trimmingCharacters(in: .whitespaces)
                line = ""
                say(said.isEmpty ? Self.prompt
                    : "\r\n\r\n  \u{1b}[2m（演示）在真实的会话中，这句话会发送给电脑上的 \(Texts.cliProduct(session.info.tool) ?? "程序")。\u{1b}[0m\r\n" + Self.prompt)
            case "\u{7f}", "\u{8}":
                if !line.isEmpty {
                    line.removeLast()
                    say("\u{8} \u{8}")
                }
            default:
                guard let a = b.unicodeScalars.first, a.value >= 0x20 else { continue }
                line.append(b)
                say(String(b))
            }
        }
        return true
    }

    func resize(cols: Int, rows: Int) {}

    func detach() {
        guard !closed else { return }
        closed = true
        onState(.closed("detached"))
    }

    /// 结束会话：和真的一样，里面的程序收到 SIGHUP 退出（128 + 1），一会儿以后连接关上
    func end() -> Bool {
        guard opened, !closed, !readOnly, session.info.canEnd else { return false }
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(400))
            guard !closed else { return }
            closed = true
            say("\r\n")
            ended()
            onExit(129)
            onState(.closed("ended"))
        }
        return true
    }
}
