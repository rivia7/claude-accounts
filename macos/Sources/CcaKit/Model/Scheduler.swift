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
    /// 点了“重新登录”之后这么久里盯着这个账号（每分钟查一次），登录一完成就看得到
    static let reloginWatch: TimeInterval = 15 * 60

    enum LoginWatch: Equatable, Sendable {
        case normal
        /// 登录已失效：多查也没用，查得很慢（在别处重新登录了靠这个恢复）
        case expired
        /// 刚点了“重新登录”，还没查到成功
        case relogging
    }

    struct Item: Equatable {
        var key: String
        var inUse: Bool
        /// 上次查到或者试过的时间（Unix 秒），0 是没查过
        var updated: Double
        var login: LoginWatch = .normal
    }

    /// 看得见时没在用的账号 8 分钟一次：比 cca 算“过期”的 10 分钟短一点，数字不会变灰。
    static func interval(inUse: Bool, visible: Bool, lowPower: Bool, login: LoginWatch = .normal) -> TimeInterval {
        let minutes: Double
        switch login {
        case .relogging: return 55   // 定时器每分钟一次，留点余量
        case .expired: minutes = visible ? 15 : 60
        case .normal: minutes = visible ? (inUse ? 5 : 8) : (inUse ? 10 : 60)
        }
        return minutes * 60 * (lowPower ? 2 : 1)
    }

    static func dueKeys(_ items: [Item], visible: Bool, lowPower: Bool, now: Double) -> [String] {
        items.filter { now - $0.updated >= interval(inUse: $0.inUse, visible: visible, lowPower: lowPower, login: $0.login) }.map(\.key)
    }
}

/// 自动检测网络出口的节奏，比查用量慢得多：出口不常变，测一次要访问好几个外部服务（出口 IP、纯净度、DNS、WebRTC）。
/// 在用的账号看得见时半小时一次、看不见时一小时；没在用的两小时、四小时（cca 算“结果过时”是 6 小时，App 开着就不会过时）；低电量模式慢一倍。
/// 用户走开了、系统过热时不测（和查用量同一个闸门）；没测成也按间隔重试。
enum ExitPolling {
    /// 一次最多测几个账号（cca 是一个接一个测的），剩下的下一分钟接着测
    static let batch = 6

    static func interval(inUse: Bool, visible: Bool, lowPower: Bool) -> TimeInterval {
        let minutes: Double = visible ? (inUse ? 30 : 120) : (inUse ? 60 : 240)
        return minutes * 60 * (lowPower ? 2 : 1)
    }

    /// 到期的账号，最久没测的排前面。
    static func dueKeys(_ items: [UsagePolling.Item], visible: Bool, lowPower: Bool, now: Double) -> [String] {
        Array(items.filter { now - $0.updated >= interval(inUse: $0.inUse, visible: visible, lowPower: lowPower) }
            .sorted { $0.updated < $1.updated }
            .prefix(batch)
            .map(\.key))
    }
}

/// 定时拉维护者的映射（`cca policy pull --json`）：低频，下次的时间是 cca 给的（`next_check_epoch`，一小时一次），App 不重算。
enum PolicyPolling {
    /// 刚启动、到期了或者没拉成时至少等这么久：不空转，也不抢着在启动时干活
    static let minDelay: TimeInterval = 5 * 60
    /// 不知道下次什么时候（还没读到 state）隔多久看一次
    static let fallback: TimeInterval = 3600

    static func delay(next: Double?, now: Double) -> TimeInterval {
        guard let next else { return minDelay }
        return max(next - now, minDelay)
    }

    static func isDue(next: Double?, now: Double) -> Bool {
        (next ?? 0) <= now
    }
}

/// 自动统计折合金额（`cca cost sync --json`，PROTOCOL 3.5）：看得见金额时（折合金额页、账号页的「折合金额」）5 分钟一次，
/// 看不见时 30 分钟一次；还没统计过（`scan.state` 是 never）时马上跑一次。不另开定时器：跟着查用量的那一下（每分钟一次）看到没到期，
/// 用户走开了、系统过热时一样不跑。上次统计的时间取 cca 给的（`scan.updated_epoch`：会话结束时 cca 自己也会统计）和 App 上次试的里晚的那个，
/// 没统计成也按间隔重试。
enum CostPolling {
    static let visibleInterval: TimeInterval = 5 * 60
    static let hiddenInterval: TimeInterval = 30 * 60

    static func interval(visible: Bool) -> TimeInterval {
        visible ? visibleInterval : hiddenInterval
    }

    /// lastAttempt：App 上次跑 `cost sync` 的时间（没跑过是 nil）；updated：cca 上次统计的时间；never：cca 说还没统计过
    static func isDue(lastAttempt: Double?, updated: Double?, never: Bool, visible: Bool, now: Double) -> Bool {
        if never && lastAttempt == nil { return true }
        let last = max(lastAttempt ?? 0, never ? 0 : updated ?? 0)
        return now - last >= interval(visible: visible)
    }
}

/// App 的定时器。都带容差，让系统把唤醒合在一起：
/// - 用量、网络出口和折合金额：每分钟看一眼该查哪些账号（只算内存里的东西，到期了才起进程，见 UsagePolling、ExitPolling、CostPolling）；
/// - 分流页打开时每 60 秒查一次 live；
/// - 账号页的「设备」看得见时马上查一次，之后每 60 秒一次（问维护者的服务器，很轻）；
/// - 客户端更新：睡到 cca 说的下一次检查时间（App 隐藏时也一样），不空转；定时器自己不发安装命令（允许自动更新时由 cca 在检查里顺带装上）。
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
    /// 离下次该拉映射还有多久（AppModel 按 cca 给的时间算）
    var policyDelay: (() -> TimeInterval)?
    var onPolicyTick: (() -> Void)?
    /// 账号页的「设备」看得见时隔多久查一次
    var devicesInterval: TimeInterval
    var onDevicesTick: (() -> Void)?

    private var policyLoop: Task<Void, Never>?
    private var usageLoop: Task<Void, Never>?
    private var liveLoop: Task<Void, Never>?
    private var clientCheckLoop: Task<Void, Never>?
    private var devicesLoop: Task<Void, Never>?

    init(usageInterval: TimeInterval = 60, liveInterval: TimeInterval = 60, clientCheckInterval: TimeInterval = 3600,
         devicesInterval: TimeInterval = 60) {
        self.usageInterval = usageInterval
        self.liveInterval = liveInterval
        self.clientCheckInterval = clientCheckInterval
        self.devicesInterval = devicesInterval
    }

    var usageRunning: Bool { usageLoop != nil }
    var liveRunning: Bool { liveLoop != nil }
    var clientChecksRunning: Bool { clientCheckLoop != nil }
    var policyPollingRunning: Bool { policyLoop != nil }
    var devicesPollingRunning: Bool { devicesLoop != nil }

    /// 账号页的「设备」看得见：马上查一次，之后每 devicesInterval 秒一次。restart：换了账号，马上再查一次
    func updateDevicesPolling(enabled: Bool, restart: Bool = false) {
        if !enabled || restart {
            devicesLoop?.cancel()
            devicesLoop = nil
        }
        if enabled, devicesLoop == nil {
            devicesLoop = loop(every: { [weak self] in self?.devicesInterval ?? 60 }, immediately: true) { [weak self] in self?.onDevicesTick?() }
        }
    }

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

    func updatePolicyPolling(enabled: Bool) {
        if enabled {
            if policyLoop == nil {
                policyLoop = loop(every: { [weak self] in self?.policyDelay?() ?? PolicyPolling.fallback }, immediately: false) { [weak self] in self?.onPolicyTick?() }
            }
        } else {
            policyLoop?.cancel()
            policyLoop = nil
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
        updatePolicyPolling(enabled: false)
        updateDevicesPolling(enabled: false)
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
