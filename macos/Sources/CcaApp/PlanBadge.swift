import SwiftUI

// 套餐的四档（cca 给的 plan_tier，App 不按套餐名猜）：
// 入门（Free、Go）灰色细字；标准（Pro、Plus、Team…）中性标签；进阶（Max、Pro 100 / 200…）银色金属标签 + ✦；
// 尊贵（Ultra、Pro 500）金色渐变 + 皇冠，头像带金圈，工位是金边的角落办公室，账号页的工牌上还有一道流光。
// 金色只给最高档（界面的规矩：颜色表达账号、状态、最高档的套餐）；银色是金属的灰，不算一种颜色。

enum TierStyle {
    static let gold = [Color(hex: 0xF8DB7A), Color(hex: 0xE9B949), Color(hex: 0xC98A20)]
    static let goldEdge = Color(hex: 0xB07A1A)
    static let goldInk = Color(hex: 0x3D2600)
    /// 头像外面的金圈、工位的金边
    static let goldRing = AngularGradient(colors: [Color(hex: 0xF8DB7A), Color(hex: 0xC98A20), Color(hex: 0xF3CF6B),
                                                   Color(hex: 0xB8791A), Color(hex: 0xF8DB7A)], center: .center)
    static let goldStroke = LinearGradient(colors: [Color(hex: 0xF3CF6B), Color(hex: 0xC98A20), Color(hex: 0xF3CF6B)],
                                           startPoint: .topLeading, endPoint: .bottomTrailing)
    static let silver = [Color(hex: 0xF6F7F9), Color(hex: 0xC9CED7)]
    static let silverEdge = Color(hex: 0x9AA1AD)
    static let silverInk = Color(hex: 0x2B2F36)
}

/// 套餐徽章。compact 用在一行字里（侧栏、菜单栏、工位、分流图），regular 用在账号页的工牌上。
struct PlanBadge: View {
    enum Size { case compact, regular }

    let plan: String
    let tier: PlanTier
    var size: Size = .regular
    /// 尊贵档每隔几秒扫过一道光：只给账号页的工牌，窗口看不见、开了“减少动态效果”时不动
    var shimmer = false

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        content
            .lineLimit(1)
            .truncationMode(.tail)
            .help(Texts.planName(plan, tier: tier))
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(Texts.planName(plan, tier: tier))
    }

    private var font: Font { size == .compact ? .caption2.weight(.semibold) : .caption.weight(.semibold) }
    private var hPad: CGFloat { size == .compact ? 6 : 8 }
    private var vPad: CGFloat { size == .compact ? 1 : 3 }

    @ViewBuilder
    private var content: some View {
        switch tier {
        case .entry:
            Text(plan).font(size == .compact ? .caption : .caption.weight(.medium)).foregroundStyle(.secondary)
        case .advanced:
            label(icon: "sparkle", ink: TierStyle.silverInk)
                .background(Capsule().fill(LinearGradient(colors: TierStyle.silver, startPoint: .top, endPoint: .bottom)))
                .overlay(Capsule().strokeBorder(TierStyle.silverEdge.opacity(0.8), lineWidth: 0.5))
        case .premium:
            label(icon: "crown.fill", ink: TierStyle.goldInk)
                .background(Capsule().fill(LinearGradient(colors: TierStyle.gold, startPoint: .topLeading, endPoint: .bottomTrailing)))
                .overlay { if shimmer { Shimmer(running: !reduceMotion).clipShape(Capsule()) } }
                .overlay(Capsule().strokeBorder(TierStyle.goldEdge.opacity(0.7), lineWidth: 0.5))
                .shadow(color: TierStyle.gold[2].opacity(size == .compact ? 0 : 0.35), radius: 3, y: 1)
        default:
            // 标准档，或者 cca 认不出的新套餐：和以前一样的中性标签
            Text(plan).font(font.weight(.medium)).foregroundStyle(.secondary)
                .padding(.horizontal, hPad).padding(.vertical, vPad)
                .background(Capsule().fill(.quaternary.opacity(0.7)))
        }
    }

    private func label(icon: String, ink: Color) -> some View {
        HStack(spacing: size == .compact ? 2 : 3) {
            Image(systemName: icon).imageScale(.small)
            Text(plan)
        }
        .font(font)
        .foregroundStyle(ink)
        .padding(.horizontal, hPad)
        .padding(.vertical, vPad)
    }
}

/// 金徽章上扫过的那道光：4.5 秒一次，扫 0.9 秒；只在扫的时候出帧（SweepSchedule），其余时间不重画。
/// running 为假（减少动态效果）、窗口看不见时不动。
private struct Shimmer: View {
    let running: Bool
    @Environment(AppModel.self) private var model

    var body: some View {
        TimelineView(SweepSchedule(paused: !running || !model.mainVisible)) { ctx in
            GeometryReader { g in
                let t = ctx.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: SweepSchedule.period) / SweepSchedule.sweep
                if running, t < 1 {
                    let w = g.size.width
                    LinearGradient(colors: [.clear, .white.opacity(0.7), .clear], startPoint: .leading, endPoint: .trailing)
                        .frame(width: w * 0.45)
                        .rotationEffect(.degrees(18))
                        .offset(x: -w * 0.45 + (w * 1.45) * t)
                }
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

/// 流光的时间表：每 period 秒扫一次，扫的 sweep 秒里每秒 30 帧，扫完补一帧（光移出去），其余时间不出帧。
private struct SweepSchedule: TimelineSchedule {
    static let period = 4.5
    static let sweep = 0.9
    let paused: Bool

    func entries(from start: Date, mode: TimelineScheduleMode) -> AnyIterator<Date> {
        if paused { return AnyIterator([start].makeIterator()) }
        let fps = mode == .lowFrequency ? 4.0 : 30.0
        var cycle = (start.timeIntervalSinceReferenceDate / Self.period).rounded(.down)
        var frame = 0
        let frames = Int(Self.sweep * fps) + 1
        return AnyIterator {
            defer {
                frame += 1
                if frame > frames { frame = 0; cycle += 1 }
            }
            return Date(timeIntervalSinceReferenceDate: cycle * Self.period + Double(frame) / fps)
        }
    }
}
