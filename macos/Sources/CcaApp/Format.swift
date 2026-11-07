import AppKit
import SwiftUI

// 颜色和小部件：品牌色、内容层的色块、状态色、头像、用量条、标签。文字本身在 CcaKit 的 Texts 里（可测试）。
//
// 颜色只表达三件事：“哪个账号”（账号色，就是启动器角标的颜色，线路、分流图里的线都用它）、
// “什么状态”（系统的绿 / 橙 / 红，正常时尽量不出现）和最高档的套餐（金色，PlanBadge.swift 的 TierStyle）。
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

/// 和 App 图标同一套颜色（tools/make-icon.swift）。
enum Brand {
    static let indigo = Color(hex: 0x2D3A9E)
    static let ink = Color(hex: 0x1A2266)
    static let night = Color(hex: 0x0C1033)
    static let teal = Color(hex: 0x19C2B0)
    /// 图标里三条线的颜色（蓝、绿、粉）
    static let lines: [Color] = [Color(hex: 0x0A84FF), Color(hex: 0x30A46C), Color(hex: 0xD6409F)]
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

/// 品牌标记：App 图标的缩小版——深靛蓝的圆角方块里，一个出口分出三条线。
struct BrandMark: View {
    var size: CGFloat = 28

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: size * 0.27, style: .continuous)
        ZStack {
            shape.fill(LinearGradient(colors: [Brand.indigo, Brand.ink, Brand.night], startPoint: .topLeading, endPoint: .bottomTrailing))
            shape.fill(RadialGradient(colors: [Brand.teal.opacity(0.5), .clear], center: .bottomTrailing,
                                      startRadius: 0, endRadius: size * 0.9))
            Canvas { ctx, s in
                let w = s.width, h = s.height
                let hub = CGPoint(x: w * 0.3, y: h * 0.5)
                let ends = [CGPoint(x: w * 0.72, y: h * 0.27), CGPoint(x: w * 0.76, y: h * 0.5), CGPoint(x: w * 0.72, y: h * 0.73)]
                let line = max(w * 0.06, 1)
                for (p, c) in zip(ends, Brand.lines) {
                    var path = Path()
                    path.move(to: hub)
                    let mid = hub.x + (p.x - hub.x) * 0.55
                    path.addCurve(to: p, control1: CGPoint(x: mid, y: hub.y), control2: CGPoint(x: mid, y: p.y))
                    ctx.stroke(path, with: .color(.white.opacity(0.85)), style: StrokeStyle(lineWidth: line, lineCap: .round))
                    let r = w * 0.09
                    ctx.fill(Path(ellipseIn: CGRect(x: p.x - r, y: p.y - r, width: r * 2, height: r * 2)), with: .color(c))
                }
                let hr = w * 0.1
                ctx.stroke(Path(ellipseIn: CGRect(x: hub.x - hr, y: hub.y - hr, width: hr * 2, height: hr * 2)),
                           with: .color(.white), lineWidth: line)
            }
        }
        .overlay(shape.strokeBorder(.white.opacity(0.12), lineWidth: 0.5))
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// 账号头像：和启动器图标右下角一样的标识（文字 + 颜色）；API 账号是钥匙。最高档的套餐（Ultra、Pro 500）外面有一圈金边
/// （画在头像外面，不改变占的位置）。
struct BadgeAvatar: View {
    let badge: Badge?
    var isAPI = false
    var size: CGFloat = 28
    var tier: PlanTier = .unknown

    init(badge: Badge?, isAPI: Bool = false, size: CGFloat = 28, tier: PlanTier = .unknown) {
        self.badge = badge
        self.isAPI = isAPI
        self.size = size
        self.tier = tier
    }

    init(account a: Account, size: CGFloat) {
        self.init(badge: a.badge, isAPI: a.isAPI, size: size, tier: a.planTier)
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
        .overlay {
            if tier == .premium {
                let w = max(size * 0.07, 1.5)
                Circle().strokeBorder(TierStyle.goldRing, lineWidth: w).padding(-(w + 1))
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// 账号的头像，在用（桌面 App 开着、命令行里有会话）时右下角有绿点，悬停说在用什么。侧栏、菜单栏、账号页共用。
struct AccountAvatar: View {
    @Environment(AppModel.self) private var model
    let account: Account
    let size: CGFloat
    /// 绿点的大小
    let dot: CGFloat

    var body: some View {
        let a = account
        BadgeAvatar(account: a, size: size)
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
