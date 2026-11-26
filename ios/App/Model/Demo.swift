import CcaRemoteKit
import Foundation

/// 演示模式（App 审核、没有电脑时自己看看）：本地模拟两台电脑和几个会话，终端里放一段预先写好的输出，敲的字只在这台手机上回显。
/// 不联网、不碰钥匙串；退出演示时全丢掉
@MainActor
final class DemoBackend: Backend {
    private var all: [Peer]
    private let started = Date().timeIntervalSince1970

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
                                  info: SessionInfo(machine: p.name, tool: "claude", name: "工作", cwd: "~/srv/api", since: t - 2 * 3600 - 640, viewOnly: false),
                                  online: true, viewers: 0),
                    RemoteSession(sid: String(repeating: "2", count: 32),
                                  info: SessionInfo(machine: p.name, tool: "codex", name: "个人", cwd: "~/notes", since: t - 25 * 60, viewOnly: true),
                                  online: true, viewers: 1),
                ]
            case "b2":
                sessions = [
                    RemoteSession(sid: String(repeating: "3", count: 32),
                                  info: SessionInfo(machine: p.name, tool: "claude", name: "账号 2", cwd: "~/Projects/site", since: t - 86400 - 3600, viewOnly: false),
                                  online: false, viewers: 0),
                ]
            default:
                sessions = []
            }
            return PeerListing(peer: p, gone: false, sessions: sessions, failure: nil)
        }
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

    func link(peer: Peer, session: RemoteSession, cols: Int, rows: Int, readOnly: Bool) -> TerminalLink {
        DemoLink(session: session, readOnly: readOnly || session.info.viewOnly)
    }

    func fingerprint(on server: ServerRef) -> String? { "SHA256:demo" }

    // 演示模式不登记推送、不碰钥匙串
    func servers() -> [ServerRef] { [] }
    func pushSet(on server: ServerRef, token: String, env: String, sids: [String]?) async -> PushSetResult { .failed(RemoteFailure("demo")) }
    func syncNoteKeys() {}
}

/// 演示的终端：一段预先写好的输出（一点一点放出来），之后是一个提示符，敲的字回显，回车后答一句“这是演示”
@MainActor
final class DemoLink: TerminalLink {
    var onOutput: (Bytes) -> Void = { _ in }
    var onSize: (Int, Int) -> Void = { _, _ in }
    var onState: (Attachment.State) -> Void = { _ in }
    var onExit: (Int32) -> Void = { _ in }
    var onNote: (Note) -> Void = { _ in }
    private let session: RemoteSession
    private let readOnly: Bool
    private var line = ""
    private var closed = false
    private var opened = false

    init(session: RemoteSession, readOnly: Bool) {
        self.session = session
        self.readOnly = readOnly
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
            for chunk in Self.script(session) {
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
