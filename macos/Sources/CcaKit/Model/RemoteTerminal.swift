import Foundation
import Observation

/// 界面里开着的一个终端（PROTOCOL 9.3）：接在一个会话上的 `cca remote attach`，跑在 App 开的伪终端上。
/// 这里只管进程和字节的进出；怎么画是界面那一层的事（它把 `sink` 接上，按键经 `write` 送回来）。
/// 终端里的内容只在内存里：还没有视图接上来时最多攒几兆，多的丢掉最早的。
@MainActor @Observable
final class RemoteTerminal: Identifiable {
    enum Status: Equatable {
        /// 还没起：等界面量好终端的大小
        case waiting
        case running
        /// 进程退出了（退出码见 PROTOCOL 9.3）
        case ended(Int32)
        /// 没起来
        case failed(String)
    }

    /// 怎么起进程：AppModel 给（`CcaClient.startTerminal`），测试里换成假的
    typealias Starter = @MainActor (_ arguments: [String], _ cols: Int, _ rows: Int,
                                    _ onEvent: @escaping @Sendable (PtyEvent) -> Void) throws -> TerminalHandle

    nonisolated let id = UUID()
    let session: RemoteSession
    /// 只看不操作（`-r`）：用户选的，或者那台机器只让看
    let readOnly: Bool
    private(set) var status: Status = .waiting
    /// 会话里的程序给终端设的标题
    var title = ""
    /// 是这边要断开的（点了「断开」、关了窗口、退出 App），不是会话自己结束、连接自己断的
    private(set) var closedByUser = false
    /// 界面那一层放它画终端的东西：终端开着就留着，换到别的页再回来画面还在
    @ObservationIgnored var surface: AnyObject?

    @ObservationIgnored private let starter: Starter
    @ObservationIgnored private var handle: TerminalHandle?
    @ObservationIgnored private var pending: [Data] = []
    @ObservationIgnored private var pendingBytes = 0
    /// 进程退出（断开了、会话结束了）时告诉 AppModel：重新查一次会话列表。没起来的不算
    @ObservationIgnored var onEnd: (@MainActor (RemoteTerminal) -> Void)?
    /// 画这块终端的视图：接上来时先把攒着的交给它
    @ObservationIgnored var sink: ((Data) -> Void)? {
        didSet {
            guard let sink else { return }
            let held = pending
            pending = []
            pendingBytes = 0
            for d in held { sink(d) }
        }
    }

    static let pendingLimit = 4 * 1024 * 1024

    init(session: RemoteSession, readOnly: Bool, starter: @escaping Starter) {
        self.session = session
        self.readOnly = readOnly
        self.starter = starter
    }

    /// `cca` 后面的参数
    var arguments: [String] { ["remote", "attach"] + (readOnly ? ["-r"] : []) + [session.id] }

    var isRunning: Bool { status == .running }
    /// 界面里的终端能不能右键粘贴（PROTOCOL 9.3）：在运行、不是只看的（只看的 cca 也会丢掉输入，这里干脆不发）
    var canPaste: Bool { isRunning && !readOnly }
    var hasEnded: Bool {
        switch status {
        case .ended, .failed: return true
        case .waiting, .running: return false
        }
    }

    /// 界面量好终端的大小以后起进程（只起一次；之后大小变了走 `resize`）。
    func begin(cols: Int, rows: Int) {
        guard status == .waiting else { return }
        do {
            // 这个回调拿着 self 不放（不是 weak）：AppModel 把这个终端换掉、关掉以后进程还要一小会儿才退出，那时还得有人收它的退出、
            // 告诉 AppModel 再查一次列表。进程退出、读的线程结束，回调跟着放掉，不会一直留着
            handle = try starter(arguments, cols, rows) { [self] event in
                // 在读伪终端的线程上：等主线程处理完这一块再读下一块（终端画不过来时，那边自然被压住）
                if Thread.isMainThread {
                    MainActor.assumeIsolated { self.receive(event) }
                } else {
                    DispatchQueue.main.sync {
                        MainActor.assumeIsolated { self.receive(event) }
                    }
                }
            }
            status = .running
        } catch {
            status = .failed(String(describing: error))
        }
    }

    private func receive(_ event: PtyEvent) {
        switch event {
        case .output(let data):
            if let sink {
                sink(data)
            } else {
                pending.append(data)
                pendingBytes += data.count
                while pendingBytes > RemoteTerminal.pendingLimit, pending.count > 1 {
                    pendingBytes -= pending.removeFirst().count
                }
            }
        case .exited(let code):
            handle = nil
            status = .ended(code)
            onEnd?(self)
        }
    }

    /// 按键、鼠标、终端对查询的回答：原样写给伪终端。
    func write(_ data: Data) {
        handle?.write(data)
    }

    func resize(cols: Int, rows: Int) {
        if status == .waiting {
            begin(cols: cols, rows: rows)
        } else {
            handle?.resize(cols: cols, rows: rows)
        }
    }

    /// 断开：接入端退出，会话在那边照常运行。
    func close() {
        guard !hasEnded else { return }
        closedByUser = true
        if status == .waiting {
            status = .ended(0)
            onEnd?(self)
        }
        handle?.close()
    }
}
