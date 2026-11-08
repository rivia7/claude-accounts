import Foundation

/// 后台干活之前看的系统状态（App 层读，测试里换掉）。
struct SystemConditions: Sendable, Equatable {
    /// 用户多久没碰键盘鼠标了（秒）
    var idleSeconds: Double = 0
    /// 开了低电量模式
    var lowPower = false
    /// 系统过热（thermalState 到了 serious）
    var thermalPressure = false
}

/// 自动刷新用量的节奏。查一次要经账号的出口起一个 Claude Code / Codex 进程，不便宜，所以只查到期的账号：
/// 在用的（桌面 App 开着、命令行里有会话，额度在变）查得勤，没在用的很少查；App 看不见时放慢，低电量模式再慢一倍；
/// 用户走开了、系统过热时不查（回来后下一分钟就补上）。查失败也按间隔重试，不会每分钟试一次。
enum UsagePolling {
    /// 这么久没碰电脑就不查了：没人看
    static let awayAfter: TimeInterval = 15 * 60

    struct Item: Equatable {
        var key: String
        var inUse: Bool
        /// 上次查到或者试过的时间（Unix 秒），0 是没查过
        var updated: Double
    }

    /// 看得见时没在用的账号 8 分钟一次：比 cca 算“过期”的 10 分钟短一点，数字不会变灰。
    static func interval(inUse: Bool, visible: Bool, lowPower: Bool) -> TimeInterval {
        let minutes: Double = visible ? (inUse ? 5 : 8) : (inUse ? 10 : 60)
        return minutes * 60 * (lowPower ? 2 : 1)
    }

    static func dueKeys(_ items: [Item], visible: Bool, lowPower: Bool, now: Double) -> [String] {
        items.filter { now - $0.updated >= interval(inUse: $0.inUse, visible: visible, lowPower: lowPower) }.map(\.key)
    }
}

/// App 的定时器。都带容差，让系统把唤醒合在一起：
/// - 用量：每分钟看一眼该查哪些账号（只算内存里的东西，到期了才起进程，见 UsagePolling）；
/// - 分流页打开时每 60 秒查一次 live；
/// - 客户端更新：睡到 cca 说的下一次检查时间（App 隐藏时也一样），不空转；它从不负责安装。
@MainActor
final class Scheduler {
    var usageInterval: TimeInterval
    var liveInterval: TimeInterval
    var onUsageTick: (() -> Void)?
    var onLiveTick: (() -> Void)?
    /// 不知道下次什么时候该检查客户端更新时，隔多久看一次
    var clientCheckInterval: TimeInterval
    /// 离下次该检查客户端更新还有多久（AppModel 按 cca 给的时间算）
    var clientCheckDelay: (() -> TimeInterval)?
    var onClientCheckTick: (() -> Void)?

    private var usageLoop: Task<Void, Never>?
    private var liveLoop: Task<Void, Never>?
    private var clientCheckLoop: Task<Void, Never>?

    init(usageInterval: TimeInterval = 60, liveInterval: TimeInterval = 60, clientCheckInterval: TimeInterval = 3600) {
        self.usageInterval = usageInterval
        self.liveInterval = liveInterval
        self.clientCheckInterval = clientCheckInterval
    }

    var usageRunning: Bool { usageLoop != nil }
    var liveRunning: Bool { liveLoop != nil }
    var clientChecksRunning: Bool { clientCheckLoop != nil }

    func updateUsagePolling(enabled: Bool) {
        if enabled {
            if usageLoop == nil {
                usageLoop = loop(every: { [weak self] in self?.usageInterval ?? 60 }, immediately: false) { [weak self] in self?.onUsageTick?() }
            }
        } else {
            usageLoop?.cancel()
            usageLoop = nil
        }
    }

    func updateClientChecks(enabled: Bool) {
        if enabled {
            if clientCheckLoop == nil {
                clientCheckLoop = loop(every: { [weak self] in self?.clientCheckDelay?() ?? self?.clientCheckInterval ?? 3600 },
                                       immediately: false) { [weak self] in self?.onClientCheckTick?() }
            }
        } else {
            clientCheckLoop?.cancel()
            clientCheckLoop = nil
        }
    }

    /// visible：窗口或面板可见；routingVisible：分流页可见（live 立刻查一次，之后每 liveInterval 秒一次）。
    func update(visible: Bool, routingVisible: Bool) {
        if visible && routingVisible {
            if liveLoop == nil {
                liveLoop = loop(every: { [weak self] in self?.liveInterval ?? 60 }, immediately: true) { [weak self] in self?.onLiveTick?() }
            }
        } else {
            liveLoop?.cancel()
            liveLoop = nil
        }
    }

    func stop() {
        update(visible: false, routingVisible: false)
        updateUsagePolling(enabled: false)
        updateClientChecks(enabled: false)
    }

    private func loop(every interval: @escaping @MainActor () -> TimeInterval, immediately: Bool,
                      _ tick: @escaping @MainActor () -> Void) -> Task<Void, Never> {
        Task { @MainActor in
            if immediately { tick() }
            while !Task.isCancelled {
                let wait = max(interval(), 0.01)
                try? await Task.sleep(for: .seconds(wait), tolerance: .seconds(wait * 0.2))
                if Task.isCancelled { break }
                tick()
            }
        }
    }
}
