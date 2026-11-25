#if DEBUG
import CcaRemoteKit
import Foundation

/// 只在调试版里：tests/ios_kit.rs 的 in_the_simulator 在模拟器里开 App，用启动参数让它自己走一遍（不点界面），结果一行一行打到 stdout：
/// - `-test-pair <链接>`：直接配对（不问那一句）→ `pair ok <配对编号>` / `pair fail <原因>`
/// - `-test-attach`：配好以后（或者本来就配过）等会话列出来，接入第一个在线的 → `state open` …
/// - `-test-type <文字>`：接上以后敲进去（\r 结尾）
/// 正式版没有这些（`#if DEBUG`）
@MainActor
enum TestDriver {
    static func run(_ model: AppModel) async {
        let args = ProcessInfo.processInfo.arguments
        func value(_ k: String) -> String? {
            guard let i = args.firstIndex(of: k), i + 1 < args.count else { return nil }
            return args[i + 1]
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
