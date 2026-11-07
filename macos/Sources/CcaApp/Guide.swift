import SwiftUI

// 给新手的引导：第一次打开时的卡片教程（一步一张，可以随时从“帮助”菜单、工具栏的问号再看），
// 页面上的提示卡片（讲一句这一页怎么看，点“知道了”就不再出现），账号页顶上“现在能不能用、下一步点什么”的一句话，
// 以及默认收起的技术细节（端口、槽位、纯净度明细这些，点“显示细节”才出来）。

/// 引导的状态：教程看过没有、关掉了哪些提示卡片、要不要显示技术细节。记在 App 的设置里（演示模式只记在内存里）。
@MainActor
@Observable
final class Guide {
    private enum Keys {
        static let done = "guide.tutorialDone"
        static let tips = "guide.dismissedTips"
        static let details = "guide.showDetails"
    }

    @ObservationIgnored private let prefs: PrefsStore
    /// 教程正在显示
    var tutorialShown = false
    /// 教程现在在第几张（从 0 起）
    var step = 0
    private(set) var dismissedTips: Set<String>
    /// 显示技术细节（端口、槽位、出口检测的明细）
    private(set) var showDetails: Bool

    init(prefs: PrefsStore) {
        self.prefs = prefs
        dismissedTips = Set((prefs.string(forKey: Keys.tips) ?? "").split(separator: ",").map(String.init))
        showDetails = prefs.bool(forKey: Keys.details)
    }

    /// 第一次连上 cca 时打开教程（看过、跳过过的不再自动打开）。
    func startIfNeeded() {
        guard !prefs.bool(forKey: Keys.done), !tutorialShown else { return }
        step = 0
        tutorialShown = true
    }

    func openTutorial() {
        step = 0
        tutorialShown = true
    }

    /// 看完或跳过：记下来，以后不再自动打开。
    func finishTutorial() {
        prefs.set(true, forKey: Keys.done)
        tutorialShown = false
    }

    func isDismissed(_ tip: String) -> Bool { dismissedTips.contains(tip) }

    func dismiss(_ tip: String) {
        dismissedTips.insert(tip)
        prefs.set(dismissedTips.sorted().joined(separator: ","), forKey: Keys.tips)
    }

    /// 帮助菜单的“重新显示提示卡片”。
    func resetTips() {
        dismissedTips = []
        prefs.set("", forKey: Keys.tips)
    }

    func toggleDetails() {
        showDetails.toggle()
        prefs.set(showDetails, forKey: Keys.details)
    }
}

// MARK: 卡片教程

/// 教程的一张卡片：上面一幅小图，下面标题和两三句话。
struct TutorialCard {
    let title: String
    let body: String
    let art: Art

    enum Art { case fan, open, quota, route }

    static let all: [TutorialCard] = [
        TutorialCard(title: "一台 Mac，好几个账号",
                     body: "cca 帮你同时用好几个 Claude、ChatGPT 账号。每个账号走自己的网络出口、用自己的登录资料，互不串号。",
                     art: .fan),
        TutorialCard(title: "打开一个账号",
                     body: "在左边选一个账号，点「打开 Claude App」。cca 会先检查出口，再用这个账号自己的资料打开；也可以点「网页版」。菜单栏上的 cca 图标里也能一键打开。",
                     art: .open),
        TutorialCard(title: "看额度还剩多少",
                     body: "每个账号有「5 小时」和「本周」两个额度。快用完时会变橙、变红，到了重置时间自动恢复。左边列表里的两根小条就是它们。",
                     art: .quota),
        TutorialCard(title: "出口和分流",
                     body: "每个账号经路由器（或这台 Mac 上的 Clash）连到自己的节点，出口 IP 也就各不相同。「节点分流」页能看到是不是都连对了，连错时点「同步路由器」就能修好。第一次用某个账号，先点「检测出口」看看。",
                     art: .route),
    ]
}

/// 卡片教程（弹层）：一步一张，下面是第几步的点、上一步 / 下一步；第一张可以跳过，最后一张是“开始使用”。
struct TutorialSheet: View {
    @Environment(Guide.self) private var guide

    var body: some View {
        let cards = TutorialCard.all
        let i = min(max(guide.step, 0), cards.count - 1)
        let card = cards[i]
        VStack(spacing: 0) {
            TutorialArt(art: card.art)
                .frame(maxWidth: .infinity)
                .frame(height: 200)
                .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Surface.tile))
                .padding([.horizontal, .top], 24)
                .id(i)
                .transition(.opacity)
            VStack(alignment: .leading, spacing: 8) {
                Text("\(i + 1) / \(cards.count)")
                    .font(.caption.weight(.medium).monospacedDigit())
                    .foregroundStyle(.secondary)
                Text(card.title)
                    .font(.title2.weight(.semibold))
                Text(card.body)
                    .font(.body)
                    .foregroundStyle(.secondary)
                    .lineSpacing(3)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 28)
            .padding(.top, 18)
            Spacer(minLength: 12)
            HStack(spacing: 10) {
                PageDots(count: cards.count, current: i)
                Spacer()
                if i == 0 {
                    Button("跳过") { guide.finishTutorial() }
                        .keyboardShortcut(.cancelAction)
                } else {
                    Button("上一步") { guide.step = i - 1 }
                }
                Button(i == cards.count - 1 ? "开始使用" : "下一步") {
                    if i == cards.count - 1 { guide.finishTutorial() } else { guide.step = i + 1 }
                }
                .buttonStyle(.glassProminent)
                .keyboardShortcut(.defaultAction)
            }
            .controlSize(.large)
            .padding(.horizontal, 24)
            .padding(.bottom, 22)
        }
        .frame(width: 560, height: 460)
        .animation(.snappy(duration: 0.25), value: guide.step)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("新手教程，第 \(i + 1) 步，共 \(cards.count) 步")
    }
}

/// 第几步的点。
struct PageDots: View {
    let count: Int
    let current: Int

    var body: some View {
        HStack(spacing: 6) {
            ForEach(0..<count, id: \.self) { i in
                Capsule()
                    .fill(i == current ? AnyShapeStyle(.primary) : AnyShapeStyle(.quaternary))
                    .frame(width: i == current ? 18 : 7, height: 7)
            }
        }
        .accessibilityHidden(true)
    }
}

/// 教程卡片上的小图：都用 App 里真实的部件和账号色画（示意，不能点）。
struct TutorialArt: View {
    let art: TutorialCard.Art

    private static let sample: [(String, UInt32)] = [("工", 0x30A46C), ("C", 0x8E4EC6), ("F", 0x0A84FF)]

    var body: some View {
        Group {
            switch art {
            case .fan: fanArt
            case .open: openArt
            case .quota: quotaArt
            case .route: routeArt
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    /// 这台 Mac 分出三条线，各连一个账号（和 App 图标同一个样子）。
    private var fanArt: some View {
        GeometryReader { g in
            let w = g.size.width, h = g.size.height
            let hub = CGPoint(x: w * 0.3, y: h * 0.5)
            let ends = [CGPoint(x: w * 0.68, y: h * 0.24), CGPoint(x: w * 0.72, y: h * 0.5), CGPoint(x: w * 0.68, y: h * 0.76)]
            ZStack {
                ForEach(0..<3, id: \.self) { k in
                    Path { p in
                        p.move(to: hub)
                        let mid = hub.x + (ends[k].x - hub.x) * 0.55
                        p.addCurve(to: ends[k], control1: CGPoint(x: mid, y: hub.y), control2: CGPoint(x: mid, y: ends[k].y))
                    }
                    .stroke(Color(hex: Self.sample[k].1), style: StrokeStyle(lineWidth: 3, lineCap: .round))
                }
                Image(systemName: "laptopcomputer")
                    .font(.system(size: 30, weight: .regular))
                    .foregroundStyle(.secondary)
                    .padding(10)
                    .background(Circle().fill(.background))
                    .position(hub)
                ForEach(0..<3, id: \.self) { k in
                    avatar(k, size: 34).position(ends[k])
                }
            }
        }
    }

    /// 一行账号和“打开 Claude App”按钮。
    private var openArt: some View {
        VStack(spacing: 14) {
            HStack(spacing: 10) {
                avatar(0, size: 34)
                VStack(alignment: .leading, spacing: 2) {
                    Text("工作").font(.headline)
                    Text("Max · US-B").font(.caption).foregroundStyle(.secondary)
                }
                Spacer(minLength: 16)
                Label("打开 Claude App", systemImage: "macwindow")
                    .font(.callout.weight(.semibold))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                    .background(Capsule().fill(Color.accentColor))
                Label("网页版", systemImage: "safari")
                    .font(.callout)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(Capsule().fill(.background))
                    .overlay(Capsule().strokeBorder(Surface.hairline, lineWidth: 0.5))
            }
            .padding(14)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(.background))
            .frame(maxWidth: 440)
        }
        .padding(.horizontal, 24)
    }

    /// 两块额度：5 小时快用完了（红），本周还早（绿）。
    private var quotaArt: some View {
        HStack(spacing: 12) {
            miniTile("5 小时", 92, "2 小时后恢复")
            miniTile("本周", 41, "周二 16:00 重置")
        }
        .padding(.horizontal, 32)
    }

    private func miniTile(_ title: String, _ pct: Double, _ reset: String) -> some View {
        let color = Palette.usage(percent: pct)
        return VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.callout.weight(.semibold)).foregroundStyle(.secondary)
            HStack(alignment: .firstTextBaseline, spacing: 1) {
                Text("\(Int(pct))").font(.system(size: 28, weight: .semibold).monospacedDigit())
                Text("%").font(.system(size: 15, weight: .semibold))
            }
            .foregroundStyle(UsageLevel(percent: pct) == .normal ? AnyShapeStyle(.primary) : AnyShapeStyle(color))
            Meter(fraction: pct / 100, color: color, height: 6)
            Text(reset).font(.caption).foregroundStyle(.secondary)
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(.background))
    }

    /// 一条线路：这台 Mac → 路由器 → 节点 → 出口。
    private var routeArt: some View {
        var stops = [RouteStop(id: 0, mark: .origin, title: "这台 Mac"), RouteStop(id: 1, title: "路由器"),
                     RouteStop(id: 2, title: "US-B"), RouteStop(id: 3, mark: .end, title: "出口 IP")]
        stops[1].detail = "这个账号的端口"
        stops[2].detail = "节点"
        stops[3].detail = "美国 · 纯净度 97"
        return RouteStrip(stops: stops, color: Color(hex: Self.sample[0].1))
            .padding(.horizontal, 20)
    }

    private func avatar(_ k: Int, size: CGFloat) -> some View {
        ZStack {
            Circle().fill(Color(hex: Self.sample[k].1))
            Text(Self.sample[k].0)
                .font(.system(size: size * 0.44, weight: .semibold, design: .rounded))
                .foregroundStyle(.white)
        }
        .frame(width: size, height: size)
    }
}

// MARK: 提示卡片

/// 提示卡片：给新手讲一句这一页怎么看；点“知道了”就不再显示（“帮助”菜单里可以重新显示）。
struct TipCard: View {
    @Environment(Guide.self) private var guide
    let id: String
    var systemImage = "lightbulb"
    let title: String
    let message: String

    var body: some View {
        if !guide.isDismissed(id) {
            let shape = RoundedRectangle(cornerRadius: 14, style: .continuous)
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: systemImage)
                    .font(.title3)
                    .foregroundStyle(.tint)
                    .frame(width: 24)
                VStack(alignment: .leading, spacing: 4) {
                    Text(title).font(.callout.weight(.semibold))
                    Text(message)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .lineSpacing(2)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 8)
                Button("知道了") {
                    withAnimation(.snappy) { guide.dismiss(id) }
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
            }
            .padding(14)
            .background(shape.fill(Color.accentColor.opacity(0.07)))
            .overlay(shape.strokeBorder(Color.accentColor.opacity(0.2), lineWidth: 0.5))
            .accessibilityElement(children: .contain)
        }
    }
}

// MARK: 下一步

/// 给新手的一句话：这个账号现在能不能用、下一步点哪个按钮。只按 cca 给的状态说话，不重算 cca 的规则；最要紧的一条排最前。
struct Advice {
    enum Level { case ok, info, warn, bad }

    let level: Level
    let text: String

    static func make(_ a: Account, usage u: UsageEntry?, live: LiveSlot?, appLabel: String, now: Double) -> Advice {
        switch a.login {
        case .loggedOut:
            return Advice(level: .warn, text: "还没登录这个账号：点「登录…」，在终端里按提示登录。")
        case .emailMismatch:
            let who = nonEmpty(a.loginEmail).map { "（现在登录的是 \($0)）" } ?? ""
            return Advice(level: .warn, text: "登录的不是这个账号\(who)：点「重新登录…」换回来。")
        default:
            break
        }
        if !a.isAPI, !a.can.app, a.canWhy["app"] == .appMissing {
            return Advice(level: .info, text: "这台 Mac 上没装 \(appLabel)：可以先用「网页版」。")
        }
        switch live?.state {
        case .closed?:
            return Advice(level: .bad, text: "这个账号在路由器上的端口连不上：先点「同步路由器」，还不行就看看路由器。")
        case .drift?:
            return Advice(level: .warn, text: "路由器给这个账号选错了节点：点「同步路由器」就能修好。")
        default:
            break
        }
        let h = a.health.flatMap { $0.applies && !$0.stale ? $0 : nil }
        if let h, let r = h.result {
            switch r {
            case .restricted, .noExit, .portClosed, .apiUnreachable:
                return Advice(level: .bad, text: "出口有问题：\(Texts.healthResult(r, region: h.restrictedRegion))。点「检测出口」再测一次。")
            default:
                break
            }
        }
        // 额度快用完了：比纯净度、泄露更要紧（马上就用不了）
        if let u = u, !u.stale {
            for (name, w) in [("5 小时", u.fiveHour), ("本周", u.sevenDay)] {
                guard let w, let p = w.utilization, !w.isReset(now: now), UsageLevel(percent: p) == .critical else { continue }
                let when = w.resetsEpoch.map { "，" + Texts.resetText(epoch: $0, now: now) } ?? ""
                return Advice(level: .warn, text: "\(name)的额度快用完了（\(Texts.percent(p))\(when)）。可以先换个账号。")
            }
        }
        if let h {
            if h.result == .lowPurity {
                return Advice(level: .warn, text: "出口 IP 的纯净度低于设定，用的时候注意风险。")
            }
            if h.leak?.hasLeak == true {
                return Advice(level: .warn, text: "检测到 DNS 或 WebRTC 泄露：别人可能看出你的真实位置，详见下面的网络出口。")
            }
        }
        if a.health == nil {
            return Advice(level: .info, text: a.isAPI ? "第一次用这个接口？点「检测出口」看看连不连得上。"
                          : "第一次用这个账号？先点「检测出口」，看看出口在哪、干不干净。")
        }
        return Advice(level: .ok, text: a.isAPI ? "一切正常。" : "一切正常，可以打开使用。")
    }
}

/// 账号页顶上的那句话：图标、颜色跟着严重程度。
struct AdviceLine: View {
    let advice: Advice

    var body: some View {
        let (icon, color): (String, Color) = {
            switch advice.level {
            case .ok: return ("checkmark.circle.fill", .green)
            case .info: return ("info.circle.fill", .accentColor)
            case .warn: return ("exclamationmark.circle.fill", .orange)
            case .bad: return ("xmark.octagon.fill", .red)
            }
        }()
        let shape = RoundedRectangle(cornerRadius: 12, style: .continuous)
        HStack(alignment: .firstTextBaseline, spacing: 9) {
            Image(systemName: icon).foregroundStyle(color)
            Text(advice.text).foregroundStyle(.primary).fixedSize(horizontal: false, vertical: true)
        }
        .font(.callout)
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(shape.fill(color.opacity(0.08)))
        .accessibilityElement(children: .combine)
    }
}
