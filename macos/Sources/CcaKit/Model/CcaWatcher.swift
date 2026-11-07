import CoreServices
import Foundation

/// 看着 `hello.home`（~/.cca）的变化，只当作“该重新跑一次 `_gui state` / `_gui hello`”的信号：从不读那些文件。
/// 收：accounts.json、codex.json、config.json、*/usage.json、*/health.json、policy/policy.json（→ state），
/// core/config.yaml（→ core：内置代理的配置，看不见时也要马上读 state，见 AppModel），bin/cca（→ hello）。
/// 不收：chrome/、desktop/ 下面的一切（浏览器和桌面 App 的资料目录一直在写）。
/// 500 毫秒内的多次变化合并成一次（hello > core > state）。看不见时只记下“脏了”（由 AppModel 处理）。
final class CcaWatcher: @unchecked Sendable {
    enum Trigger: Sendable, Equatable {
        case state
        case hello
        /// 内置代理的配置变了（cca 刚切到内置代理、要内核、关掉了）：和 state 一样重新读，但看不见时也读
        case core

        /// 合并时留哪个：hello 最大（它之后会接着读 state），core 次之
        var rank: Int {
            switch self {
            case .state: return 0
            case .core: return 1
            case .hello: return 2
            }
        }
    }

    private static let statePaths: Set<String> = ["accounts.json", "codex.json", "config.json", "policy/policy.json"]

    /// 一个变化的路径该触发什么（纯函数，测试用）。homes 是 ~/.cca 的几种写法（原样、解析过链接的）。
    static func classify(path: String, homes: [String]) -> Trigger? {
        for home in homes {
            if let rel = relative(path, to: home) { return classify(relative: rel) }
        }
        return nil
    }

    static func classify(relative rel: String) -> Trigger? {
        let comps = rel.split(separator: "/", omittingEmptySubsequences: true).map(String.init)
        guard !comps.isEmpty else { return nil }
        if comps.contains("chrome") || comps.contains("desktop") { return nil }
        let clean = comps.joined(separator: "/")
        if clean == "bin/cca" { return .hello }
        if clean == "core/config.yaml" { return .core }
        if statePaths.contains(clean) { return .state }
        if comps.count >= 2, let last = comps.last, last == "usage.json" || last == "health.json" { return .state }
        return nil
    }

    static func relative(_ path: String, to home: String) -> String? {
        var h = home
        while h.count > 1 && h.hasSuffix("/") { h.removeLast() }
        guard path.hasPrefix(h + "/") else { return nil }
        return String(path.dropFirst(h.count + 1))
    }

    let home: String
    private let homes: [String]
    private let debounce: TimeInterval
    private let latency: CFTimeInterval
    private let handler: @Sendable (Trigger) -> Void
    private let queue = DispatchQueue(label: "local.cca.app.watcher")
    private let lock = NSLock()
    private var stream: FSEventStreamRef?
    private var pending: Trigger?
    private var fire: DispatchWorkItem?

    init(home: String, debounce: TimeInterval = 0.5, latency: CFTimeInterval = 0.2, handler: @escaping @Sendable (Trigger) -> Void) {
        self.home = home
        let resolved = URL(fileURLWithPath: home).resolvingSymlinksInPath().path
        homes = resolved == home ? [home] : [home, resolved]
        self.debounce = debounce
        self.latency = latency
        self.handler = handler
    }

    deinit {
        stop()
    }

    func start() {
        guard stream == nil else { return }
        var ctx = FSEventStreamContext(version: 0, info: Unmanaged.passUnretained(self).toOpaque(),
                                       retain: nil, release: nil, copyDescription: nil)
        let callback: FSEventStreamCallback = { _, info, count, paths, _, _ in
            guard let info else { return }
            let me = Unmanaged<CcaWatcher>.fromOpaque(info).takeUnretainedValue()
            let list = Unmanaged<CFArray>.fromOpaque(paths).takeUnretainedValue() as NSArray
            var out: [String] = []
            out.reserveCapacity(count)
            for p in list { if let s = p as? String { out.append(s) } }
            me.received(out)
        }
        let flags = FSEventStreamCreateFlags(kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagUseCFTypes
                                             | kFSEventStreamCreateFlagNoDefer)
        guard let s = FSEventStreamCreate(kCFAllocatorDefault, callback, &ctx, homes as CFArray,
                                          FSEventStreamEventId(kFSEventStreamEventIdSinceNow), latency, flags) else { return }
        FSEventStreamSetDispatchQueue(s, queue)
        if FSEventStreamStart(s) {
            stream = s
        } else {
            FSEventStreamInvalidate(s)
            FSEventStreamRelease(s)
        }
    }

    func stop() {
        lock.lock()
        let s = stream
        stream = nil
        fire?.cancel()
        fire = nil
        pending = nil
        lock.unlock()
        guard let s else { return }
        FSEventStreamStop(s)
        FSEventStreamInvalidate(s)
        FSEventStreamRelease(s)
    }

    /// FSEvents 的回调（在 queue 上）。
    private func received(_ paths: [String]) {
        var t: Trigger? = nil
        for p in paths {
            if let x = CcaWatcher.classify(path: p, homes: homes), x.rank > (t?.rank ?? -1) { t = x }
        }
        guard let t else { return }
        let handler = self.handler
        lock.lock()
        if t.rank > (pending?.rank ?? -1) { pending = t }
        fire?.cancel()
        let item = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.lock.lock()
            let p = self.pending
            self.pending = nil
            self.fire = nil
            self.lock.unlock()
            if let p { handler(p) }
        }
        fire = item
        lock.unlock()
        queue.asyncAfter(deadline: .now() + debounce, execute: item)
    }
}
