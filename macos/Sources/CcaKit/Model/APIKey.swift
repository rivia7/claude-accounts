import Foundation

// API key（PROTOCOL 4.10）：明文只在点「显示」「复制」时经管道向 cca 要一次（`cca [codex] key 账号 --show`），
// 不进被观察的状态、提示、读屏和日志；用完就丢。界面平时只显示 cca 给的掩码（`Account.apiKey.hint`）。

/// 从 cca 读到的 API key 明文。打印、调试输出、dump 里都只显示「<密文>」。
struct APIKeyValue: Sendable, Equatable, CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    let text: String

    /// cca 打到 stdout 的 key：原样、不换行，只收可见的 ASCII（8–4096 个，和 cca 的 SECRET_RE 一样）；别的都不当 key。
    init?(cca stdout: Data) {
        guard (8...4096).contains(stdout.count), stdout.allSatisfy({ (0x21...0x7e).contains($0) }),
              let s = String(data: stdout, encoding: .ascii) else { return nil }
        text = s
    }

    var description: String { "<密文>" }
    var debugDescription: String { "<密文>" }
    var customMirror: Mirror { Mirror(self, children: [:]) }
}

/// 账号页上 API key 那一行点没点开（这一行自己的状态，不放进 AppModel）。被观察的只有 phase；明文在 @ObservationIgnored 里，
/// 收起来（再点一下、30 秒后、换了账号、离开页面）时丢掉。读的结果晚到（已经收起来、换了账号）就不要了。
@MainActor @Observable
final class KeyReveal: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    enum Phase: Equatable { case hidden, loading, shown }

    private(set) var phase: Phase = .hidden
    @ObservationIgnored private(set) var secret: APIKeyValue?
    @ObservationIgnored private var generation = 0
    @ObservationIgnored private var hideTask: Task<Void, Never>?
    @ObservationIgnored private let autoHide: Duration

    init(autoHide: Duration = .seconds(30)) {
        self.autoHide = autoHide
    }

    /// 点眼睛：开着、正在读的收起来；收着的就读一次（read 读不到时自己提示原因、返回 nil）。
    func toggle(read: @escaping @MainActor () async -> APIKeyValue?) {
        if phase != .hidden { hide(); return }
        generation += 1
        let g = generation
        phase = .loading
        Task { [weak self] in
            let v = await read()
            guard let self, self.generation == g else { return }
            guard let v else { self.hide(); return }
            self.secret = v
            self.phase = .shown
            self.scheduleHide(g)
        }
    }

    func hide() {
        generation += 1
        hideTask?.cancel()
        hideTask = nil
        secret = nil
        phase = .hidden
    }

    private func scheduleHide(_ g: Int) {
        hideTask?.cancel()
        let delay = autoHide
        hideTask = Task { [weak self] in
            try? await Task.sleep(for: delay)
            guard !Task.isCancelled, let self, self.generation == g else { return }
            self.hide()
        }
    }

    nonisolated var description: String { "KeyReveal(<密文>)" }
    nonisolated var debugDescription: String { "KeyReveal(<密文>)" }
    nonisolated var customMirror: Mirror { Mirror(self, children: [:]) }
}

/// 剪贴板（App 里是 NSPasteboard，测试里是假的）。
@MainActor protocol SecretPasteboard: AnyObject {
    var changeCount: Int { get }
    /// 写进去，并标成机密（org.nspasteboard.ConcealedType：剪贴板管理器不记）；返回写完以后的 changeCount
    func writeSecret(_ s: String) -> Int
    func clear()
}

/// 复制密文：写进剪贴板，过一会儿（默认 60 秒）剪贴板还是它就清掉（期间复制了别的就不动）；退出 App 前也清一次。
@MainActor
final class SecretClipboard {
    private let board: SecretPasteboard
    private let delay: Duration
    private var pending: (count: Int, task: Task<Void, Never>)?

    init(board: SecretPasteboard, delay: Duration = .seconds(60)) {
        self.board = board
        self.delay = delay
    }

    func copy(_ s: String) {
        pending?.task.cancel()
        let count = board.writeSecret(s)
        let delay = delay
        let task = Task { [weak self] in
            try? await Task.sleep(for: delay)
            guard !Task.isCancelled else { return }
            self?.clearIfUnchanged(count)
        }
        pending = (count, task)
    }

    /// 退出 App 前：剪贴板还是复制的那个 key 就清掉。
    func flush() {
        guard let p = pending else { return }
        p.task.cancel()
        clearIfUnchanged(p.count)
    }

    private func clearIfUnchanged(_ count: Int) {
        if board.changeCount == count { board.clear() }
        pending = nil
    }
}
