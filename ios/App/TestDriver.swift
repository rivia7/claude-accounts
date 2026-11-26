#if DEBUG
import CcaRemoteKit
import Foundation

/// 只在调试版里：tests/ios_kit.rs 的 in_the_simulator 在模拟器里开 App，用启动参数让它自己走一遍（不点界面），结果一行一行打到 stdout：
/// - `-test-pair <链接>`：直接配对（不问那一句）→ `pair ok <配对编号>` / `pair fail <原因>`
/// - `-test-attach`：配好以后（或者本来就配过）等会话列出来，接入第一个在线的 → `state open` …
/// - `-test-type <文字>`：接上以后敲进去（\r 结尾）
/// - `-test-note`：会话推来的通知不真的弹（不要权限），说出来 → `note 标题|副标题|正文`
/// - `-test-wheel`：（敲完以后）等会话开了鼠标上报，当成手指往下拖了三行 → `wheel`（会话那头收到三格往上的滚轮）
/// - `-test-push-token <十六进制>`：不向 Apple 要推送令牌，用这个；当作允许发通知、不弹那一问。配好以后在服务器上登记 →
///   `push 服务器 ok|原因`，通知扩展读得到的通知密钥 → `keys 几把 配对编号…`；最后等 App 的临时目录里出现 pushed.json（测试放进来的
///   推送服务收到的那一份），像通知扩展那样解开 → `pushed 标题|副标题|正文|会话编号`（解不开是 `pushed fail`）
/// 正式版没有这些（`#if DEBUG`）
@MainActor
enum TestDriver {
    private static var args: [String] { ProcessInfo.processInfo.arguments }

    private static func value(_ k: String) -> String? {
        guard let i = args.firstIndex(of: k), i + 1 < args.count else { return nil }
        return args[i + 1]
    }

    /// Pusher 起来之前
    static func prepare() {
        guard value("-test-push-token") != nil else { return }
        Pusher.shared.testing = true
        Pusher.shared.spy = { say($0) }
    }

    static func run(_ model: AppModel) async {
        if args.contains("-test-note") {
            Notifier.spy = { say("note \($0)|\($1)|\($2)") }
        }
        if let token = value("-test-push-token") {
            Pusher.shared.testToken(token)
        }
        if let link = value("-test-pair") {
            guard let inv = PairInvite(link) else { return say("pair fail link") }
            model.pair(inv)
            while true {
                try? await Task.sleep(for: .milliseconds(100))
                switch model.pairing {
                case .done(let p): say("pair ok \(p.pid)")
                case .failed(_, let m): return say("pair fail \(m)")
                default: continue
                }
                break
            }
            model.cancelPairing()
            if value("-test-push-token") != nil {
                // 通知扩展那样读（只在共用的分组里找）
                let keys = NoteKeychain.read(group: NoteKeychain.group)
                say("keys \(keys.count) " + keys.map(\.pid).sorted().joined(separator: " "))
            }
        }
        guard args.contains("-test-attach") else { return }
        var target: (Peer, RemoteSession)?
        for _ in 0..<60 where target == nil {
            await model.fetch()
            for p in model.peers {
                if let s = model.listing(p)?.sessions.first(where: \.online) {
                    target = (p, s)
                    break
                }
            }
            if target == nil { try? await Task.sleep(for: .milliseconds(500)) }
        }
        guard let (peer, session) = target else { return say("attach fail no_session") }
        say("session \(session.sid)")
        model.attach(session, on: peer, readOnly: false)
        guard let t = model.terminal else { return }
        var last = ""
        for _ in 0..<300 {
            let now = "\(t.state)"
            if now != last {
                say("state \(stateWord(t.state))")
                last = now
            }
            if t.isOpen { break }
            if t.isClosed { return }
            try? await Task.sleep(for: .milliseconds(100))
        }
        if let text = value("-test-type") {
            try? await Task.sleep(for: .milliseconds(300))
            t.send(Bytes((text + "\r").utf8))
            say("typed")
        }
        if args.contains("-test-wheel") {
            var rolled = false
            for _ in 0..<300 {
                if TerminalSurface.of(t).testDrag(rows: 3) {
                    rolled = true
                    break
                }
                try? await Task.sleep(for: .milliseconds(100))
            }
            say(rolled ? "wheel" : "wheel fail")
            guard rolled else { return }
        }
        if value("-test-push-token") != nil {
            await openPushed()
        }
    }

    /// 测试把推送服务收到的那一份放进 App 的临时目录：像通知扩展那样拿通知密钥解开、做成要弹的字
    private static func openPushed() async {
        let file = FileManager.default.temporaryDirectory.appendingPathComponent("pushed.json")
        for _ in 0..<600 {
            if let d = try? Data(contentsOf: file), let info = (try? JSONSerialization.jsonObject(with: d)) as? [String: Any] {
                guard let n = CcaRemoteKit.openPushed(info, keys: NoteKeychain.read(group: NoteKeychain.group)), let c = NoteCard(n) else {
                    return say("pushed fail")
                }
                return say("pushed \(c.title)|\(c.subtitle)|\(c.body)|\(n.sid)")
            }
            try? await Task.sleep(for: .milliseconds(100))
        }
        say("pushed fail no_file")
    }

    private static func stateWord(_ s: Attachment.State) -> String {
        switch s {
        case .connecting: return "connecting"
        case .open: return "open"
        case .retrying: return "retrying"
        case .closed(let why): return "closed " + why
        }
    }

    private static func say(_ s: String) {
        print("cca-test: " + s)
        fflush(stdout)
    }
}
#endif
