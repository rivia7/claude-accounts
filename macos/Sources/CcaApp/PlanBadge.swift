import SwiftUI

// 套餐的四档（cca 给的 plan_tier，App 不按套餐名猜），参考 Google 2025 年后的图标语言：一个主色、颜色互相渐变、圆润扁平。
// 同一种胶囊，前面一个小徽记，徽记的形状和颜色一档比一档足：
// 入门（Free、Go）灰色圆圈；标准（Pro、Plus、Team…）蓝色圆点、淡蓝底；进阶（Max、Pro 100 / 200…）紫色菱形、淡紫底；
// 尊贵（Ultra、Pro 500）整块 Logo 的渐变（蓝 → 紫 → 玫红 → 琥珀）、白色四角星，头像有同色的光晕（BadgeAvatar），
// 总览的卡片、账号页的卡片、插画里的桌子镶同一条渐变的边。

enum TierStyle {
    /// 尊贵档的边（卡片、插画）：Logo 的渐变绕一圈
    static let edge = AngularGradient(colors: Brand.spectrumLoop, center: .center, angle: .degrees(-45))
    /// 尊贵档的横线（账号页卡片顶上那条带子的下沿）
    static let line = LinearGradient(colors: Brand.spectrum, startPoint: .leading, endPoint: .trailing)
}

/// 档位的小徽记：圈 → 圆 → 菱形 → 四角星。onColor：画在尊贵档整块渐变的底上（白色）。
struct TierMark: View {
    let tier: PlanTier
    var size: CGFloat = 11
    var onColor = false

    var body: some View {
        Canvas { c, s in
            let w = s.width, r = CGRect(origin: .zero, size: s)
            let diagonal = (CGPoint.zero, CGPoint(x: w, y: w))
            switch tier {
            case .entry:
                c.stroke(Path(ellipseIn: r.insetBy(dx: w * 0.14, dy: w * 0.14)), with: .color(Color(hex: 0x8E95A1)), lineWidth: w * 0.16)
            case .standard:
                c.fill(Path(ellipseIn: r.insetBy(dx: w * 0.08, dy: w * 0.08)),
                       with: .linearGradient(Gradient(colors: [Color(hex: 0x6FA0FF), Brand.blue]), startPoint: diagonal.0, endPoint: diagonal.1))
            case .advanced:
                var d = Path()
                d.addRoundedRect(in: r.insetBy(dx: w * 0.17, dy: w * 0.17), cornerSize: CGSize(width: w * 0.16, height: w * 0.16), style: .continuous,
                                 transform: CGAffineTransform(translationX: w / 2, y: w / 2).rotated(by: .pi / 4).translatedBy(x: -w / 2, y: -w / 2))
                c.fill(d, with: .linearGradient(Gradient(colors: [Color(hex: 0xA58BFF), Brand.violet]), startPoint: diagonal.0, endPoint: diagonal.1))
            case .premium:
                // 圆润的四角星：四条边向里弯
                let m = w / 2
                let tips = [CGPoint(x: m, y: 0), CGPoint(x: w, y: m), CGPoint(x: m, y: w), CGPoint(x: 0, y: m)]
                var star = Path()
                star.move(to: tips[0])
                for i in 0..<4 {
                    let a = tips[i], b = tips[(i + 1) % 4]
                    star.addQuadCurve(to: b, control: CGPoint(x: m + (a.x + b.x - w) * 0.12, y: m + (a.y + b.y - w) * 0.12))
                }
                star.closeSubpath()
                c.fill(star, with: onColor ? .color(.white)
                       : .linearGradient(Gradient(colors: Brand.spectrum), startPoint: CGPoint(x: 0, y: w), endPoint: CGPoint(x: w, y: 0)))
            default:
                break
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// 套餐徽章。compact 用在一行字里（侧栏、菜单栏、总览的卡片、分流图），regular 用在账号页的卡片上。
struct PlanBadge: View {
    enum Size { case compact, regular }

    let plan: String
    let tier: PlanTier
    var size: Size = .regular

    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let compact = size == .compact
        HStack(spacing: compact ? 3 : 4) {
            if tier.isKnown && tier != .unknown {
                TierMark(tier: tier, size: compact ? 9 : 11, onColor: tier == .premium)
            }
            Text(plan)
                .font(.system(size: compact ? 10.5 : 12, weight: .semibold))
                .foregroundStyle(ink)
                .lineLimit(1)
                .truncationMode(.tail)
        }
        .padding(.horizontal, compact ? 6.5 : 8.5)
        .frame(height: compact ? 17 : 21)
        .background(Capsule().fill(fill))
        .help(Texts.planName(plan, tier: tier))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Texts.planName(plan, tier: tier))
    }

    private var dark: Bool { scheme == .dark }

    private var ink: Color {
        switch tier {
        case .premium: return .white
        case .standard: return dark ? Color(hex: 0xA9C6FF) : Color(hex: 0x1D4FC4)
        case .advanced: return dark ? Color(hex: 0xC8BBFF) : Color(hex: 0x5133D6)
        default: return dark ? Color(hex: 0xA3A9B3) : Color(hex: 0x5D636D)   // 入门，或者 cca 认不出的新套餐
        }
    }

    private var fill: AnyShapeStyle {
        switch tier {
        case .premium:
            return AnyShapeStyle(LinearGradient(colors: Brand.spectrum, startPoint: .leading, endPoint: .trailing))
        case .standard:
            return AnyShapeStyle(Brand.blue.opacity(dark ? 0.22 : 0.11))
        case .advanced:
            return AnyShapeStyle(LinearGradient(colors: [Brand.violet.opacity(dark ? 0.28 : 0.14), Brand.blue.opacity(dark ? 0.22 : 0.10)],
                                                startPoint: .leading, endPoint: .trailing))
        default:
            return AnyShapeStyle(Color.primary.opacity(dark ? 0.08 : 0.05))
        }
    }
}
