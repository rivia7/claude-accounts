import AppKit
import SwiftUI

// 颜色和小部件：品牌色、内容层的色块、状态色、头像、用量条、标签。文字本身在 CcaKit 的 Texts 里（可测试）。
//
// 颜色只表达三件事：“哪个账号”（账号色，就是启动器角标的颜色，线路、分流图里的线都用它）、
// “什么状态”（系统的绿 / 橙 / 红，正常时尽量不出现）和套餐的档位（PlanBadge.swift：灰、蓝、紫，最高档是 Logo 的渐变）。
// 品牌色（App 图标的深靛蓝）只用在品牌标记和分流图的面板上。

extension Color {
    /// `#RRGGBB`；格式不对时用灰色。
    init(badge: Badge?) {
        if let (r, g, b) = badge?.rgb {
            self = Color(red: r, green: g, blue: b)
        } else {
            self = Color(nsColor: .systemGray)
        }
    }

    init(hex: UInt32) {
        self = Color(.sRGB, red: Double((hex >> 16) & 0xFF) / 255, green: Double((hex >> 8) & 0xFF) / 255,
                     blue: Double(hex & 0xFF) / 255, opacity: 1)
    }
}

/// 和 App 图标同一套颜色（tools/make-icon.swift）：Logo 的主干是蓝色，三支分别流向玫红、青绿、琥珀（颜色互相渐变过去，参考 Google
/// 2025 年后的图标语言）。最高档的套餐（标签、头像的光晕、卡片的边）用 spectrum：Logo 的颜色连成的一条渐变。
enum Brand {
    static let blue = Color(hex: 0x3B7BFF)
    static let violet = Color(hex: 0x7C5CFF)
    static let rose = Color(hex: 0xEC4C8C)
    static let teal = Color(hex: 0x14B8A6)
    static let amber = Color(hex: 0xF5A524)
    /// 蓝 → 紫 → 玫红 → 琥珀
    static let spectrum: [Color] = [blue, violet, rose, amber]
    /// 绕一圈回到蓝（环、光晕用：首尾接得上）
    static let spectrumLoop: [Color] = [blue, violet, rose, amber, blue]
}

/// 内容层的色块（不是玻璃）：深浅色下都比背景深一点 / 亮一点。
enum Surface {
    /// 额度砖、分流图的面板、引导卡片
    static let tile = Color.primary.opacity(0.045)
    /// 用量条的底槽
    static let track = Color.primary.opacity(0.09)
    static let hairline = Color.primary.opacity(0.12)
}

enum Palette {
    /// 用量的颜色档：固定的语义色（不用强调色：强调色是橙色、红色时三档会分不清）。
    static func usage(_ level: UsageLevel) -> Color {
        switch level {
        case .normal: return .green
        case .warning: return .orange
        case .critical: return .red
        }
    }

    static func usage(percent: Double) -> Color { usage(UsageLevel(percent: percent)) }

    static func slot(_ s: SlotState?) -> Color {
        switch s {
        case .some(.ok): return .green
        case .some(.drift): return .orange
        case .some(.closed): return .red
        case .some(.empty): return .secondary
        default: return .gray
        }
    }

    static func health(_ r: HealthResult?) -> Color {
        switch r {
        case .some(.ok), .some(.apiOk): return .green
        case .some(.lowPurity), .some(.noScore): return .orange
        case .some(.restricted), .some(.noExit), .some(.portClosed), .some(.apiUnreachable): return .red
        default: return .gray
        }
    }

    static func leak(_ s: LeakState?) -> Color {
        switch s {
        case .some(.ok): return .green
        case .some(.leak): return .red
        case .some(.failed): return .orange
        default: return .secondary
        }
    }
}

/// 品牌标记：App 图标里那个字形，不带底（Google 新图标的做法：形状自己站着）——粗圆的一根线分成三支，
/// 主干蓝色，每一支从蓝渐变到自己的颜色。和 tools/make-icon.swift 画的是同一个形。
struct BrandMark: View {
    var size: CGFloat = 28

    var body: some View {
        Canvas { ctx, s in
            let w = s.width, weight = w * 0.15
            let root = CGPoint(x: w * 0.12, y: w * 0.5), fork = CGPoint(x: w * 0.38, y: w * 0.5)
            // 分开之前都是蓝（接头处叠着看不出），分开以后才流向自己的颜色；中间放饱和的颜色（蓝直接到琥珀会发灰），
            // 最下面那支走完整条 Logo 渐变。中间那支先画
            let branches: [(CGPoint, [Gradient.Stop])] = [
                (CGPoint(x: w * 0.86, y: w * 0.5), [.init(color: Brand.blue, location: 0.3), .init(color: Color(hex: 0x22A3E6), location: 0.65),
                                                    .init(color: Brand.teal, location: 1)]),
                (CGPoint(x: w * 0.82, y: w * 0.17), [.init(color: Brand.blue, location: 0.3), .init(color: Brand.violet, location: 0.65),
                                                     .init(color: Brand.rose, location: 1)]),
                (CGPoint(x: w * 0.82, y: w * 0.83), [.init(color: Brand.blue, location: 0.28), .init(color: Brand.violet, location: 0.52),
                                                     .init(color: Brand.rose, location: 0.76), .init(color: Brand.amber, location: 1)]),
            ]
            for (tip, stops) in branches {
                var p = Path()
                p.move(to: fork)
                let k = fork.x + (tip.x - fork.x) * 0.5
                p.addCurve(to: tip, control1: CGPoint(x: k, y: fork.y), control2: CGPoint(x: k, y: tip.y))
                ctx.stroke(p, with: .linearGradient(Gradient(stops: stops), startPoint: fork, endPoint: tip),
                           style: StrokeStyle(lineWidth: weight, lineCap: .round, lineJoin: .round))
            }
            var stem = Path()
            stem.move(to: root)
            stem.addLine(to: fork)
            ctx.stroke(stem, with: .color(Brand.blue), style: StrokeStyle(lineWidth: weight, lineCap: .round))
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// “总览”的图标：同一套颜色的 2×2 圆角方块（每块一个颜色，从浅到深）。
struct OverviewMark: View {
    var size: CGFloat = 22

    var body: some View {
        Canvas { ctx, s in
            let w = s.width, gap = w * 0.12, b = (w - gap) / 2
            let colors = [Brand.blue, Brand.rose, Brand.teal, Brand.amber]
            for (i, c) in colors.enumerated() {
                let r = CGRect(x: CGFloat(i % 2) * (b + gap), y: CGFloat(i / 2) * (b + gap), width: b, height: b)
                ctx.fill(Path(roundedRect: r, cornerRadius: b * 0.32, style: .continuous),
                         with: .linearGradient(Gradient(colors: [c.mix(with: .white, by: 0.25), c]), startPoint: r.origin,
                                               endPoint: CGPoint(x: r.maxX, y: r.maxY)))
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// 账号头像：和启动器图标右下角一样的标识（文字 + 颜色）；API 账号是钥匙。最高档的套餐（Ultra、Pro 500）有光晕（参考 Gemini：
/// 背后一团几种颜色互相流动的柔光，外面一圈清楚的渐变环）；光晕画在头像外面，不改变占的位置。
struct BadgeAvatar: View {
    let badge: Badge?
    var isAPI = false
    var size: CGFloat = 28
    var tier: PlanTier = .unknown
    /// 光晕转到的角度（在用时 AccountAvatar 让它慢慢转）
    var haloPhase: Double = 0

    init(badge: Badge?, isAPI: Bool = false, size: CGFloat = 28, tier: PlanTier = .unknown, haloPhase: Double = 0) {
        self.badge = badge
        self.isAPI = isAPI
        self.size = size
        self.tier = tier
        self.haloPhase = haloPhase
    }

    init(account a: Account, size: CGFloat, haloPhase: Double = 0) {
        self.init(badge: a.badge, isAPI: a.isAPI, size: size, tier: a.planTier, haloPhase: haloPhase)
    }

    var body: some View {
        ZStack {
            Circle().fill(badge == nil ? Color(nsColor: .systemGray).opacity(0.6) : Color(badge: badge))
            if let b = badge, !b.label.isEmpty {
                Text(b.label)
                    .font(.system(size: size * 0.44, weight: .semibold, design: .rounded))
                    .foregroundStyle(.white)
                    .minimumScaleFactor(0.5)
                    .lineLimit(1)
                    .padding(size * 0.1)
            } else {
                Image(systemName: isAPI ? "key.horizontal.fill" : "person.fill")
                    .font(.system(size: size * 0.4, weight: .semibold))
                    .foregroundStyle(.white)
            }
        }
        .overlay(Circle().strokeBorder(.white.opacity(0.18), lineWidth: 0.5))
        .frame(width: size, height: size)
        .background {
            if tier == .premium { Halo(size: size, phase: haloPhase) }
        }
        .accessibilityHidden(true)
    }
}

/// 最高档头像的光晕：一圈清楚的渐变环，环和头像之间一道真正透明的缝（选中的行、彩色的底上都对），
/// 环外面一团模糊开的同色柔光。
private struct Halo: View {
    let size: CGFloat
    let phase: Double

    var body: some View {
        let ring = max(size * 0.055, 1.5), gap = max(size * 0.045, 1.3)
        let outer = size + 2 * (gap + ring), glow = size * 0.16
        let colors = AngularGradient(colors: Brand.spectrumLoop, center: .center, angle: .degrees(phase))
        ZStack {
            Circle().fill(colors)
                .frame(width: outer + glow, height: outer + glow)
                .blur(radius: glow * 0.9)
                .opacity(0.7)
                .frame(width: outer + 4 * glow, height: outer + 4 * glow)
                // 柔光只在环外面：环和头像之间的缝是空的
                .mask(Annulus(hole: outer - ring).fill(style: FillStyle(eoFill: true)))
            Circle().strokeBorder(colors, lineWidth: ring)
                .frame(width: outer, height: outer)
        }
        .allowsHitTesting(false)
    }
}

/// 整块减去中间一个圆（直径 hole），用 eoFill 填
private struct Annulus: Shape {
    let hole: CGFloat

    func path(in r: CGRect) -> Path {
        var p = Path(r)
        p.addEllipse(in: CGRect(x: r.midX - hole / 2, y: r.midY - hole / 2, width: hole, height: hole))
        return p
    }
}

/// 账号的头像，在用（桌面 App 开着、命令行里有会话）时右下角有绿点，悬停说在用什么；最高档的光晕在用时慢慢流动
/// （6 秒一圈；所在的窗口、面板看不见或者开了“减少动态效果”时不动）。侧栏、菜单栏、账号页共用。
struct AccountAvatar: View {
    @Environment(AppModel.self) private var model
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let account: Account
    let size: CGFloat
    /// 绿点的大小
    let dot: CGFloat
    /// 所在的窗口 / 面板看得见
    var visible = true

    var body: some View {
        let a = account
        let flowing = a.isPremium && a.isWorking && visible && !reduceMotion
        TimelineView(.animation(minimumInterval: 1.0 / 24, paused: !flowing)) { ctx in
            BadgeAvatar(account: a, size: size,
                        haloPhase: flowing ? (ctx.date.timeIntervalSinceReferenceDate / 6).truncatingRemainder(dividingBy: 1) * 360 : 0)
        }
        .frame(width: size, height: size)
        .overlay(alignment: .bottomTrailing) {
                if a.isWorking {
                    let what = Texts.working(a, appLabel: model.appLabel(a.tool), now: Date().timeIntervalSince1970)
                    RunningDot(size: dot)
                        .offset(x: 1, y: 1)
                        .help(what)
                        .accessibilityLabel(what)
                }
            }
    }
}

/// 头像右下角的绿点：这个账号在用（桌面 App 开着，或者有经 cca 启动的命令行会话）。
struct RunningDot: View {
    var size: CGFloat = 9

    var body: some View {
        Circle().fill(Color.green)
            .frame(width: size, height: size)
            .overlay(Circle().stroke(.background, lineWidth: max(size * 0.18, 1.5)))
    }
}

/// 一根用量条。dim 时（数据旧了）用灰色，不靠透明度。
struct Meter: View {
    var fraction: Double
    var color: Color
    var height: CGFloat = 8
    var dim = false

    var body: some View {
        GeometryReader { g in
            ZStack(alignment: .leading) {
                Capsule().fill(Surface.track)
                if fraction > 0 {
                    Capsule()
                        .fill(dim ? Color.secondary.opacity(0.6) : color)
                        .frame(width: max(g.size.width * min(fraction, 1), height))
                }
            }
        }
        .frame(height: height)
        .accessibilityHidden(true)
    }
}

/// 一个小标签（套餐、状态）：文字一律 primary / secondary（对比度够），颜色只用在前面的圆点或图标上。
struct Tag: View {
    let text: String
    /// 圆点 / 图标的颜色；nil 是中性的标签（文字用 secondary）。
    var color: Color?
    var systemImage: String?

    var body: some View {
        HStack(spacing: 4) {
            if let systemImage {
                Image(systemName: systemImage).foregroundStyle(color ?? .secondary)
            } else if let color {
                Circle().fill(color).frame(width: 6, height: 6)
            }
            Text(text).foregroundStyle(color == nil ? Color.secondary : Color.primary)
        }
        .font(.caption.weight(.medium))
        .lineLimit(1)
        .padding(.horizontal, 8)
        .padding(.vertical, 3)
        .background(Capsule().fill(.quaternary.opacity(0.7)))
        .accessibilityElement(children: .combine)
    }
}

/// 侧栏、菜单栏账号第二行的一段：文字、套餐（画成徽章）、节点（放不下时先省掉）。
enum SubtitlePart: Hashable {
    case text(String)
    case plan(String)
    case node(String)

    var text: String {
        switch self {
        case .text(let s), .plan(let s), .node(let s): return s
        }
    }
}

extension Account {
    /// 侧栏、菜单栏的第二行：套餐 · 节点；没登录时先说登录状态；API 账号：接口 · 直连 / 节点。画的（AccountSubtitle）和读屏念的
    /// （subtitle）都按这一份。
    var subtitleParts: [SubtitlePart] {
        if isAPI { return [.text(api?.host ?? "第三方 API"), .node(node ?? "直连")] }
        var parts: [SubtitlePart] = []
        if login != .loggedIn { parts.append(.text(Texts.login(login, loginEmail: nil))) }
        if let p = nonEmpty(plan) { parts.append(.plan(p)) }
        if let n = node { parts.append(.node(n)) }
        return parts
    }

    var subtitle: String { subtitleParts.map(\.text).joined(separator: " · ") }

    /// 这个账号的颜色（启动器角标的颜色）：头像、线路、分流图里的线都用它；没有角标的（第三方 API 账号）是灰色。
    var lineColor: Color { badge == nil ? Color(nsColor: .systemGray) : Color(badge: badge) }

    /// 登录有问题时的颜色（侧栏、菜单栏第二行前面的点）；登录正常时为 nil（不显示）。
    var loginIssueColor: Color? {
        switch login {
        case .loggedOut: return .gray
        case .emailMismatch: return .orange
        default: return nil
        }
    }
}

/// 侧栏、菜单栏账号的第二行（按 Account.subtitleParts）：套餐画成徽章；放不下时先不写节点（只剩一个省略号没有意义），徽章不截断。
/// 读屏照旧念 subtitle。
struct AccountSubtitle: View {
    let account: Account

    var body: some View {
        let a = account
        let parts = a.subtitleParts
        ViewThatFits(in: .horizontal) {
            line(a, parts)
            line(a, parts.filter { if case .node = $0 { return false }; return true })
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .lineLimit(1)
        .frame(height: 17)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(a.subtitle)
    }

    private func line(_ a: Account, _ parts: [SubtitlePart]) -> some View {
        HStack(spacing: 4) {
            if let c = a.loginIssueColor {
                Circle().fill(c).frame(width: 6, height: 6).padding(.trailing, 1)
            }
            ForEach(Array(parts.enumerated()), id: \.offset) { i, p in
                if i > 0 { Text("·") }
                if case .plan(let s) = p {
                    PlanBadge(plan: s, tier: a.planTier, size: .compact).fixedSize().layoutPriority(1)
                } else {
                    Text(p.text)
                }
            }
        }
    }
}
