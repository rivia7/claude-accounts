import Foundation

/// 只在看得见的时候跑的定时器：窗口或菜单栏面板可见时每 15 分钟刷新用量；分流页打开时每 60 秒查一次 live。
/// 客户端更新检查有独立的开关，App 隐藏时也按间隔检查；它从不负责安装。
@MainActor
final class Scheduler {
    var usageInterval: TimeInterval
    var liveInterval: TimeInterval
    var onUsageTick: (() -> Void)?
    var onLiveTick: (() -> Void)?
    var clientCheckInterval: TimeInterval
    var onClientCheckTick: (() -> Void)?

    private var usageLoop: Task<Void, Never>?
    private var liveLoop: Task<Void, Never>?
    private var clientCheckLoop: Task<Void, Never>?

    init(usageInterval: TimeInterval = 15 * 60, liveInterval: TimeInterval = 60, clientCheckInterval: TimeInterval = 60) {
        self.usageInterval = usageInterval
        self.liveInterval = liveInterval
        self.clientCheckInterval = clientCheckInterval
    }

    var usageRunning: Bool { usageLoop != nil }
    var liveRunning: Bool { liveLoop != nil }
    var clientChecksRunning: Bool { clientCheckLoop != nil }

    func updateClientChecks(enabled: Bool) {
        if enabled {
            if clientCheckLoop == nil {
                clientCheckLoop = loop(every: clientCheckInterval, immediately: false) { [weak self] in self?.onClientCheckTick?() }
            }
        } else {
            clientCheckLoop?.cancel()
            clientCheckLoop = nil
        }
    }

    /// visible：窗口或面板可见；routingVisible：分流页可见（live 立刻查一次，之后每 liveInterval 秒一次）。
    func update(visible: Bool, routingVisible: Bool) {
        if visible {
            if usageLoop == nil {
                usageLoop = loop(every: usageInterval, immediately: false) { [weak self] in self?.onUsageTick?() }
            }
        } else {
            usageLoop?.cancel()
            usageLoop = nil
        }
        if visible && routingVisible {
            if liveLoop == nil {
                liveLoop = loop(every: liveInterval, immediately: true) { [weak self] in self?.onLiveTick?() }
            }
        } else {
            liveLoop?.cancel()
            liveLoop = nil
        }
    }

    func stop() {
        update(visible: false, routingVisible: false)
        updateClientChecks(enabled: false)
    }

    private func loop(every interval: TimeInterval, immediately: Bool, _ tick: @escaping @MainActor () -> Void) -> Task<Void, Never> {
        Task { @MainActor in
            if immediately { tick() }
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: UInt64(max(interval, 0.01) * 1_000_000_000))
                if Task.isCancelled { break }
                tick()
            }
        }
    }
}
