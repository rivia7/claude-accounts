import SwiftUI

// 套餐的四档（cca 给的 plan_tier，App 不按套餐名猜）。徽章不抢眼：和 Tag 一样的中性胶囊、secondary 的字，
// 档位只看前面的小徽记，一档比一档足：入门灰色圆圈、标准蓝色圆点、进阶紫色菱形、尊贵（Ultra、Pro 500）Logo 渐变的四角星。
// Logo 的整条渐变（蓝 → 紫 → 玫红 → 琥珀）只给最高档：这颗星，和侧栏里它头像的光晕（BadgeAvatar）。画法都在 TierStyle。

enum TierStyle {
    /// Logo 的渐变绕一圈（首尾都是蓝，接得上）：侧栏头像的光晕用 0°
    static func ring(_ degrees: Double) -> AngularGradient {
        AngularGradient(colors: Brand.spectrumLoop, center: .center, angle: .degrees(degrees))
    }
    /// Canvas 里最高档的徽记：在 r 里从左上到右下
    static func shading(_ r: CGRect) -> GraphicsContext.Shading {
        .linearGradient(spectrum, startPoint: r.origin, endPoint: CGPoint(x: r.maxX, y: r.maxY))
    }
    private static let spectrum = Gradient(colors: Brand.spectrum)
}

/// 档位的小徽记：圈 → 圆 → 菱形 → 四角星。
private struct TierMark: View {
    let tier: PlanTier
    let size: CGFloat

    var body: some View {
        Canvas { c, s in
            let w = s.width, r = CGRect(origin: .zero, size: s)
            switch tier {
            case .entry:
                c.stroke(Path(ellipseIn: r.insetBy(dx: w * 0.14, dy: w * 0.14)), with: .color(Color(hex: 0x8E95A1)), lineWidth: w * 0.16)
            case .standard:
                c.fill(Path(ellipseIn: r.insetBy(dx: w * 0.08, dy: w * 0.08)), with: Self.tint(Brand.blue, w))
            case .advanced:
                var d = Path()
                d.addRoundedRect(in: r.insetBy(dx: w * 0.17, dy: w * 0.17), cornerSize: CGSize(width: w * 0.16, height: w * 0.16), style: .continuous,
                                 transform: CGAffineTransform(translationX: w / 2, y: w / 2).rotated(by: .pi / 4).translatedBy(x: -w / 2, y: -w / 2))
                c.fill(d, with: Self.tint(Brand.violet, w))
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
                c.fill(star, with: TierStyle.shading(r))
            default:
                break
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }

    /// 从左上的浅色到右下的本色（和总览的图标一样的做法）
    private static func tint(_ c: Color, _ w: CGFloat) -> GraphicsContext.Shading {
        .linearGradient(Gradient(colors: [c.mix(with: .white, by: 0.25), c]), startPoint: .zero, endPoint: CGPoint(x: w, y: w))
    }
}

/// 套餐徽章：中性的胶囊和字（和 Tag 一样），颜色只在前面的小徽记上。compact 用在一行字里（侧栏、菜单栏、总览的卡片、分流图），
/// regular 用在账号页名字下面那排标签里（和旁边的 Tag 一样高）。
struct PlanBadge: View {
    enum Size {
        case compact, regular
        /// 胶囊的高（侧栏、菜单栏账号的第二行按 compact 的高排：AccountSubtitle）
        var height: CGFloat { self == .compact ? 17 : 19 }
    }

    let plan: String
    let tier: PlanTier
    var size: Size = .regular

    var body: some View {
        let compact = size == .compact
        HStack(spacing: compact ? 3 : 4) {
            if tier.isKnown && tier != .unknown {
                TierMark(tier: tier, size: compact ? 9 : 10)
            }
            Text(plan)
                .font(.caption.weight(.medium))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.tail)
        }
        .padding(.horizontal, compact ? 6.5 : 8)
        .frame(height: size.height)
        .background(Capsule().fill(.quaternary.opacity(0.7)))
        .help(Texts.planName(plan, tier: tier))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Texts.planName(plan, tier: tier))
    }
}
