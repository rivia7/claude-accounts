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

    init() {
        let server = ServerRef(url: "https://demo.example.com", pins: [])!
        let now = Date().timeIntervalSince1970
        all = [
            Peer(pid: String(repeating: "a1", count: 16), fp: "SHA256:demo-studio", name: "工作室的 Mac mini", at: now - 86400 * 9, server: server, team: nil),
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
                    RemoteSession(sid: String(repeating: "1", count: 32),
                                  info: SessionInfo(machine: p.name, tool: "claude", name: "工作", cwd: "~/srv/api", since: t - 2 * 3600 - 640, viewOnly: false,
                                                    aid: DemoAccounts.aid(1)),
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
            return PeerListing(peer: p, gone: false, sessions: sessions + launched.filter { $0.pid == p.pid }.map(\.session), failure: nil,
                               status: DemoAccounts.status(p.pid, machine: p.name, now: Date().timeIntervalSince1970), statusServed: true,
                               launcher: DemoLaunch.launcher(p))
        }
    }

    func launch(peer: Peer, launcher: RemoteLauncher, template: LaunchTemplate, cols: Int, rows: Int, line: String?) async -> LaunchResult {
        try? await Task.sleep(for: .milliseconds(900))
        guard launcher.online else { return .failed(RemoteFailure("nohost")) }
        let sid = (0..<16).map { _ in String(format: "%02x", UInt8.random(in: 0...255)) }.joined()
        let info = SessionInfo(machine: peer.name, tool: template.tool, name: template.account, cwd: template.dir,
                               since: Date().timeIntervalSince1970, viewOnly: false, aid: template.aid)
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
        return DemoLink(session: session, readOnly: readOnly || session.info.viewOnly, launched: mine != nil, line: mine?.line)
    }

    func fingerprint(on server: ServerRef) -> String? { "SHA256:demo" }

    // 演示模式不登记推送、不碰钥匙串
    func servers() -> [ServerRef] { [] }
    func pushSet(on server: ServerRef, token: String, env: String, sids: [String]?, usage: Bool?) async -> PushSetResult {
        .failed(RemoteFailure("demo"))
    }
    func syncNoteKeys() {}
}

/// 演示的「新建会话」：工作室的 Mac mini 上写好了三个模板（两个能带开场白），MacBook Air 的启动器不在线
enum DemoLaunch {
    static func launcher(_ p: Peer) -> RemoteLauncher? {
        switch p.pid.prefix(2) {
        case "a1":
            let t = [
                LaunchTemplate(id: "0a1b2c3d", digest: "0123456789abcdef", name: "修 api 的 bug", tool: "claude", account: "工作",
                               aid: DemoAccounts.aid(1), dir: "~/srv/api", args: "--model opus", line: true),
                LaunchTemplate(id: "1b2c3d4e", digest: "123456789abcdef0", name: "整理笔记", tool: "codex", account: "个人",
                               aid: DemoAccounts.aid(2), dir: "~/notes", args: "", line: true),
                LaunchTemplate(id: "2c3d4e5f", digest: "23456789abcdef01", name: "看看网站", tool: "claude", account: "备用",
                               aid: DemoAccounts.aid(3), dir: "~/Projects/site", args: "--permission-mode plan", line: false),
            ]
            return RemoteLauncher(sid: String(repeating: "f1", count: 16), info: LauncherInfo(machine: p.name, templates: t), online: true)
        case "b2":
            let t = [
                LaunchTemplate(id: "3d4e5f60", digest: "3456789abcdef012", name: "写周报", tool: "claude", account: "账号 2",
                               aid: DemoAccounts.aid(4), dir: "~/Documents", args: "", line: true),
            ]
            return RemoteLauncher(sid: String(repeating: "f2", count: 16), info: LauncherInfo(machine: p.name, templates: t), online: false)
        default:
            return nil
        }
    }
}

/// 演示的账号（「账号」页）：两台电脑上的几个账号，用量、重置时间照现在算。在用的、额度用完了的、建议先用的、登录失效的、
/// Antigravity 的几组额度、Codex 还能重置额度，各有一个
enum DemoAccounts {
    /// 第 n 个账号的编号、会话说明里的账号摘要
    static func id(_ n: Int) -> String { String(repeating: String(format: "%x", n), count: 16) }
    static func aid(_ n: Int) -> String { String(repeating: String(format: "c%x", n), count: 16) }

    static func status(_ pid: String, machine: String, now: Double) -> HostStatus? {
        let accounts: [[String: Any]]
        switch pid.prefix(2) {
        case "a1": accounts = [work(now), personal(now), spare(now)]
        case "b2": accounts = [second(now), gemini(now)]
        default: return nil
        }
        // 和真的一样经 JSON 读一遍（数、true / false 的样子和服务器给的一样）
        let doc: [String: Any] = ["v": 1, "machine": machine, "accounts": accounts, "at": now - 180]
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

    /// 在用，Max 20x：5 小时 42%，本周 63%（Fable 另外计），记了 11 周
    private static func work(_ now: Double) -> [String: Any] {
        let weekly = now + 3 * 86400 + 4 * 3600
        return ["id": id(1), "aid": aid(1), "tool": "claude", "name": "工作", "kind": "oauth", "login": "logged_in", "plan": "Max 20x",
                "tier": "premium", "busy": true, "color": "#8E4EC6", "badge": "工",
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
                "usage": usage(now, ["five_hour": window(100, in: 65 * 60, now), "seven_day": window(71, in: weekly - now, now),
                                     "reset_credits": ["availableCount": 2, "next_expires_epoch": (now + 9 * 86400).rounded()]]),
                "weeks": weeks([45, 62, 80, 57, 91], current: 71, end: weekly.rounded())]
    }

    /// 空闲，本周快重置了、还剩不少：建议先用
    private static func spare(_ now: Double) -> [String: Any] {
        let weekly = now + 20 * 3600
        return ["id": id(3), "aid": aid(3), "tool": "claude", "name": "备用", "kind": "oauth", "login": "logged_in", "plan": "Pro",
                "tier": "standard", "busy": false, "color": "#0A84FF", "badge": "备",
                "usage": usage(now, ["five_hour": window(8, in: 4 * 3600 + 20 * 60, now), "seven_day": window(41, in: weekly - now, now),
                                     "use_first": ["left_percent": 59, "from_epoch": now - 3600, "until_epoch": weekly.rounded()]]),
                "weeks": weeks([33, 52], current: 41, end: weekly.rounded())]
    }

    /// 登录失效了（用的是以前查到的）
    private static func second(_ now: Double) -> [String: Any] {
        let weekly = now + 2 * 86400
        return ["id": id(4), "aid": aid(4), "tool": "claude", "name": "账号 2", "kind": "oauth", "login": "logged_in", "plan": "Max",
                "tier": "advanced", "busy": false, "color": "#E5484D", "badge": "2",
                "usage": ["source": "cache", "fetched_epoch": now - 5 * 3600, "stale": true, "error_code": "auth",
                          "five_hour": window(12, in: -3600, now), "seven_day": window(35, in: weekly - now, now)],
                "weeks": weeks([60, 44, 71], current: 35, end: weekly.rounded())]
    }

    /// Antigravity：两组额度
    private static func gemini(_ now: Double) -> [String: Any] {
        func bucket(_ used: Double, _ seconds: Double, _ slot: String) -> [String: Any] {
            window(used, in: seconds, now).merging(["slot": slot]) { $1 }
        }
        return ["id": id(5), "tool": "agy", "name": "Gemini", "kind": "oauth", "login": "logged_in", "plan": "Google AI Pro",
                "tier": "standard", "busy": false, "color": "#F76B15", "badge": "G",
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
            (80, "\u{1b}[2J\u{1b}[H\(dim)演示：这是一屏示意，不是真的会话。敲的字只在这台手机上回显，不发到任何地方。\(off)\r\n\r\n"),
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
            (200, s.info.viewOnly ? "\r\n\(dim)（只看：这个会话那台电脑只让看）\(off)\r\n" : prompt),
        ]
    }

    /// 「新建会话」刚起的会话：工具、文件夹，有开场白的先放它，然后是提示符
    private static func fresh(_ s: RemoteSession, line: String?) -> [(delay: Int, text: String)] {
        let dim = "\u{1b}[2m", off = "\u{1b}[0m", violet = "\u{1b}[38;5;141m", bold = "\u{1b}[1m"
        let tool = Texts.cliProduct(s.info.tool) ?? "会话"
        var out: [(delay: Int, text: String)] = [
            (80, "\u{1b}[2J\u{1b}[H\(dim)演示：这是一屏示意，不是真的会话。敲的字只在这台手机上回显，不发到任何地方。\(off)\r\n\r\n"),
            (400, "\(violet)\(bold)● \(tool)\(off)  \(dim)\(s.info.cwd)\(off)\r\n\r\n"),
        ]
        if let line {
            out.append((300, "\(violet)›\(off) \(line)\r\n\r\n"))
            out.append((600, "  \(dim)（演示）真的会话里，开场白是会话起来以后收到的头一句话，\(tool) 接着就开始做。\(off)\r\n"))
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
                    : "\r\n\r\n  \u{1b}[2m（演示）真的会话里，这一句会发给那台电脑上的 \(Texts.cliProduct(session.info.tool) ?? "程序")。\u{1b}[0m\r\n" + Self.prompt)
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
}
