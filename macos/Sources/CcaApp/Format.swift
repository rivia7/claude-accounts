import AppKit
import SwiftUI

// 颜色和小部件：品牌色、内容层的色块、状态色、头像、用量条、标签。文字本身在 CcaKit 的 Texts 里（可测试）。
// 颜色表达什么、给谁用，见 macos/README.md「界面的规矩」；套餐档位的样子在 PlanBadge.swift（TierStyle）。

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

/// Logo 的颜色（取 LogoGlyph 的，App 图标也是这一份）。最高档的套餐用 spectrum：Logo 的颜色连成的一条渐变。
enum Brand {
    static let blue = Color(hex: LogoGlyph.blue)
    static let violet = Color(hex: LogoGlyph.violet)
    static let rose = Color(hex: LogoGlyph.rose)
    static let teal = Color(hex: LogoGlyph.teal)
    static let amber = Color(hex: LogoGlyph.amber)
    /// 蓝 → 紫 → 玫红 → 琥珀
    static let spectrum: [Color] = [blue, violet, rose, amber]
    /// 绕一圈回到蓝（环、光晕用：首尾接得上）
    static let spectrumLoop: [Color] = spectrum + [blue]
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

/// 品牌标记：App 图标里那个字形（LogoGlyph），不带底（Google 新图标的做法：形状自己站着）。
struct BrandMark: View {
    var size: CGFloat = 28

    var body: some View {
        Canvas { ctx, s in
            ctx.withCGContext { LogoGlyph.draw(in: $0, rect: CGRect(origin: .zero, size: s)) }
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

/// 账号头像：和启动器图标右下角一样的标识（文字 + 颜色）；API 账号是钥匙。最高档的套餐（Ultra、Pro 500）只在侧栏里有光晕：
/// 一道细的渐变环，环外一层很淡的柔光。光晕画在头像外面、不改变占的位置，最远伸出 haloRoom：放它的地方得留出这么宽，
/// 不然会被裁掉（侧栏的一格只到头像的边）。
struct BadgeAvatar: View {
    let badge: Badge?
    var isAPI = false
    var size: CGFloat = 28
    /// 画光晕（init(account:) 按套餐算好：要了、又是最高档才画）
    var halo = false
    /// 光晕慢慢流动（AccountAvatar：账号在用、能动的时候）
    var haloFlowing = false

    /// 光晕伸出头像边缘多远（含柔光）
    static func haloRoom(_ size: CGFloat) -> CGFloat { HaloMetrics(size).room }

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
            if halo { Halo(size: size, flowing: haloFlowing) }
        }
        .accessibilityHidden(true)
    }
}

extension BadgeAvatar {
    /// halo：这里要不要光晕（只有侧栏要）；画不画还看套餐是不是最高档
    init(account a: Account, size: CGFloat, halo: Bool = false, haloFlowing: Bool = false) {
        self.init(badge: a.badge, isAPI: a.isAPI, size: size, halo: halo && a.isPremium, haloFlowing: haloFlowing)
    }
}

/// 光晕的尺寸（按头像的大小）：头像和环之间一道透明的缝，细的环，环外柔光的模糊半径；room 是连柔光一共伸出头像边缘多远。
private struct HaloMetrics {
    let gap, ring, glow, room: CGFloat

    init(_ size: CGFloat) {
        gap = max(size * 0.04, 1)
        ring = max(size * 0.045, 1.2)
        glow = size * 0.05
        room = (gap + ring + glow * 1.6).rounded(.up)
    }
}

/// 最高档头像的光晕。流动（6 秒一圈）是转整个光晕：除了渐变它都是圆的，转它就是转渐变；
/// 转的是画好的一层（HaloLayer），每帧只改一个旋转角，不重画模糊。
private struct Halo: View {
    let size: CGFloat
    let flowing: Bool

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 24, paused: !flowing)) { ctx in
            HaloLayer(size: size)
                .rotationEffect(.degrees(flowing ? (ctx.date.timeIntervalSinceReferenceDate / 6).truncatingRemainder(dividingBy: 1) * 360 : 0))
        }
        .allowsHitTesting(false)
    }
}

/// 不转的光晕：一道细的渐变环，环和头像之间一道真正透明的缝（选中的行、彩色的底上都对），环外面淡淡晕开一点同色的光。
private struct HaloLayer: View {
    let size: CGFloat

    var body: some View {
        let m = HaloMetrics(size)
        let outer = size + 2 * (m.gap + m.ring), box = size + 2 * m.room
        ZStack {
            Circle().strokeBorder(TierStyle.ring(0), lineWidth: m.ring * 2)
                .frame(width: outer, height: outer)
                .blur(radius: m.glow)
                .opacity(0.4)
                .frame(width: box, height: box)
                // 柔光只在环外面（缝是空的），外边在 room 处裁成圆：转起来裁的地方不变，也碰不到侧栏格子的直边
                .clipShape(Circle().subtracting(Circle().inset(by: m.room - m.gap - m.ring / 2)))
            Circle().strokeBorder(TierStyle.ring(0), lineWidth: m.ring)
                .frame(width: outer, height: outer)
                .opacity(0.85)
        }
        .drawingGroup()
    }
}

/// 账号的头像，在用（桌面 App 开着、命令行里有会话）时右下角有绿点，悬停说在用什么。侧栏、菜单栏、账号页共用；
/// 只有侧栏要最高档的光晕（halo），在用时慢慢流动（只在 motionAllowed 时：所在的窗口、面板看得见，没开“减少动态效果”）。
struct AccountAvatar: View {
    @Environment(AppModel.self) private var model
    @Environment(\.motionAllowed) private var motionAllowed
    let account: Account
    let size: CGFloat
    /// 绿点的大小
    let dot: CGFloat
    var halo = false

    var body: some View {
        let a = account
        BadgeAvatar(account: a, size: size, halo: halo, haloFlowing: halo && a.isWorking && motionAllowed)
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
    /// 登录已失效（查用量时才知道）：先说这个
    var expired = false

    var body: some View {
        let a = account
        // 登录失效时不写套餐：徽章不截断，放在一行里会把这句话挤没
        let parts = expired ? [SubtitlePart.text(Texts.usageError(.auth))] + a.subtitleParts.filter { if case .plan = $0 { return false }; return true }
            : a.subtitleParts
        ViewThatFits(in: .horizontal) {
            line(a, parts)
            line(a, parts.filter { if case .node = $0 { return false }; return true })
        }
        .font(.caption)
        .foregroundStyle(.plainSecondary)
        .lineLimit(1)
        .frame(height: PlanBadge.Size.compact.height)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(parts.map(\.text).joined(separator: " · "))
    }

    private func line(_ a: Account, _ parts: [SubtitlePart]) -> some View {
        HStack(spacing: 4) {
            if let c = a.loginIssueColor ?? (expired ? .orange : nil) {
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
