import CcaRemoteKit
import SwiftUI

// 样子照 Mac App（macos/README.md「界面的规矩」）：Logo 的颜色只取 LogoGlyph（和 Mac App 共用 macos/Sources/CcaApp/LogoGlyph.swift
// 那一个文件）；颜色只表达状态（系统的绿 / 橙 / 红，正常时尽量不出现）；内容层是实的色块（Surface），玻璃只给控件（按钮、浮着的条）；
// 不为装饰上色、画框。这里的几样是 Mac App 里同名的那几样搬过来的（Mac 的在 Format.swift、Glass.swift），只留手机上用得到的。

extension Color {
    init(hex: UInt32) {
        self = Color(.sRGB, red: Double((hex >> 16) & 0xFF) / 255, green: Double((hex >> 8) & 0xFF) / 255,
                     blue: Double(hex & 0xFF) / 255, opacity: 1)
    }
}

/// Logo 的颜色（取 LogoGlyph 的，App 图标也是这一份）
enum Brand {
    static let blue = Color(hex: LogoGlyph.blue)
    static let violet = Color(hex: LogoGlyph.violet)
    static let rose = Color(hex: LogoGlyph.rose)
    static let teal = Color(hex: LogoGlyph.teal)
    static let amber = Color(hex: LogoGlyph.amber)
    /// Logo 的整条渐变：只给最高档套餐的四角星（PlanBadge）
    static let spectrum = [blue, violet, rose, amber]
}

extension Color {
    /// 「#RRGGBB」（账号头像的颜色，那台电脑给的）
    init?(hexString s: String) {
        guard s.utf8.count == 7, s.hasPrefix("#"), let v = UInt32(s.dropFirst(), radix: 16) else { return nil }
        self.init(hex: v)
    }
}

/// 内容层的色块（不是玻璃）：深浅色下都比背景深一点 / 亮一点
enum Surface {
    static let tile = Color.primary.opacity(0.045)
    /// 用量条的底槽
    static let track = Color.primary.opacity(0.09)
    static let hairline = Color.primary.opacity(0.12)
}

/// 用量的颜色档（和 Mac App 的 Palette.usage 一样：固定的语义色）：70% 以下绿、70–89% 橙、90% 以上红
enum Palette {
    static func usage(percent: Double) -> Color {
        percent >= 90 ? .red : percent >= 70 ? .orange : .green
    }
}

/// 用量条（Mac App 的 Meter）：灰色的底槽，按用了多少填上颜色；数据旧了填灰色（不靠透明度）
struct Meter: View {
    var fraction: Double
    var color: Color
    var height: CGFloat = 6
    var dim = false

    /// 数据旧了、不全时填的灰色（每周用量里没记全的那一周也是它）
    static let dimColor = Color.secondary.opacity(0.6)

    var body: some View {
        GeometryReader { g in
            ZStack(alignment: .leading) {
                Capsule().fill(Surface.track)
                if fraction > 0 {
                    Capsule()
                        .fill(dim ? Self.dimColor : color)
                        .frame(width: max(g.size.width * min(fraction, 1), height))
                }
            }
        }
        .frame(height: height)
        .accessibilityHidden(true)
    }
}

/// 账号的头像（Mac App 的 BadgeAvatar）：启动器的颜色和上面的字（那台电脑给的）；没有时是灰色的人像，第三方 API 是钥匙。
/// 在用（那台电脑上桌面 App 开着、命令行里有会话）时右下角一个绿点
struct AccountAvatar: View {
    let account: AccountStatus
    var size: CGFloat = 34
    /// 在用时画绿点（总览的插画里的脸不画）
    var dot = true

    var body: some View {
        ZStack {
            Circle().fill(account.color.flatMap { Color(hexString: $0) } ?? Color(uiColor: .systemGray).opacity(0.6))
            if !account.badge.isEmpty {
                Text(account.badge)
                    .font(.system(size: size * 0.44, weight: .semibold, design: .rounded))
                    .foregroundStyle(.white)
                    .minimumScaleFactor(0.5)
                    .lineLimit(1)
                    .padding(size * 0.1)
            } else {
                Image(systemName: account.isAPI ? "key.horizontal.fill" : "person.fill")
                    .font(.system(size: size * 0.4, weight: .semibold))
                    .foregroundStyle(.white)
            }
        }
        .overlay(Circle().strokeBorder(.white.opacity(0.18), lineWidth: 0.5))
        .frame(width: size, height: size)
        .overlay(alignment: .bottomTrailing) {
            if dot, account.busy {
                Circle().fill(.green)
                    .frame(width: max(size * 0.28, 8), height: max(size * 0.28, 8))
                    .overlay(Circle().strokeBorder(Color(uiColor: .systemBackground), lineWidth: 1.5))
                    .offset(x: 1, y: 1)
            }
        }
        .accessibilityHidden(true)
    }
}

/// 套餐档位的小徽记（Mac App 的 TierMark，一档比一档足）：entry 灰色圆圈、standard 蓝色圆点、advanced 紫色菱形、premium Logo 渐变的四角星。
/// 套餐徽章、「账号」页按套餐筛的胶囊都用它（档位界面上不写字，只有这个样子）
struct TierMark: View {
    let tier: String
    let size: CGFloat

    var body: some View {
        Canvas { c, s in
            let w = s.width, r = CGRect(origin: .zero, size: s)
            func tint(_ color: Color) -> GraphicsContext.Shading {
                .linearGradient(Gradient(colors: [color.mix(with: .white, by: 0.25), color]), startPoint: .zero, endPoint: CGPoint(x: w, y: w))
            }
            switch tier {
            case "entry":
                c.stroke(Path(ellipseIn: r.insetBy(dx: w * 0.14, dy: w * 0.14)), with: .color(Color(hex: 0x8E95A1)), lineWidth: w * 0.16)
            case "standard":
                c.fill(Path(ellipseIn: r.insetBy(dx: w * 0.08, dy: w * 0.08)), with: tint(Brand.blue))
            case "advanced":
                var d = Path()
                d.addRoundedRect(in: r.insetBy(dx: w * 0.17, dy: w * 0.17), cornerSize: CGSize(width: w * 0.16, height: w * 0.16), style: .continuous,
                                 transform: CGAffineTransform(translationX: w / 2, y: w / 2).rotated(by: .pi / 4).translatedBy(x: -w / 2, y: -w / 2))
                c.fill(d, with: tint(Brand.violet))
            case "premium":
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
                c.fill(star, with: .linearGradient(Gradient(colors: Brand.spectrum), startPoint: .zero, endPoint: CGPoint(x: w, y: w)))
            default:
                break
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// 套餐徽章（Mac App 的 PlanBadge）：中性的胶囊和字，颜色只在前面的小徽记上（档位是那台电脑上的 cca 给的 plan_tier，不按名字猜）
struct PlanBadge: View {
    let plan: String
    let tier: String

    var body: some View {
        HStack(spacing: 4) {
            if ["entry", "standard", "advanced", "premium"].contains(tier) {
                TierMark(tier: tier, size: 9)
            }
            Text(plan).font(.caption.weight(.medium)).foregroundStyle(.secondary).lineLimit(1)
        }
        .padding(.horizontal, 7)
        .frame(height: 18)
        .background(Capsule().fill(.quaternary.opacity(0.7)))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Texts.planName(plan))
    }
}

/// 品牌标记：App 图标里那个字形，不带底
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

/// “远程会话”的图标（和 Mac App 侧栏里的一样）：终端的提示符，紫渐变到玫红
struct RemoteMark: View {
    var size: CGFloat = 22

    var body: some View {
        Canvas { ctx, s in
            let w = s.width
            func p(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: x * w, y: y * w) }
            var g = Path()
            g.move(to: p(0.16, 0.24)); g.addLine(to: p(0.46, 0.50)); g.addLine(to: p(0.16, 0.76))
            g.move(to: p(0.56, 0.78)); g.addLine(to: p(0.86, 0.78))
            let shade = Gradient(stops: [.init(color: Brand.violet.mix(with: .white, by: 0.25), location: 0),
                                         .init(color: Brand.violet, location: 0.35), .init(color: Brand.rose, location: 1)])
            ctx.stroke(g, with: .linearGradient(shade, startPoint: p(0.15, 0), endPoint: p(0.85, 1)),
                       style: StrokeStyle(lineWidth: w * 0.15, lineCap: .round, lineJoin: .round))
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// 「总览」的图标（Mac App 的 OverviewMark）：同一套颜色的 2×2 圆角方块（每块一个颜色，从浅到深）
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

/// 「用量统计」「折合金额」「共享与同步」的图标（Mac App 的 StatsMark、CostMark、SharingMark）：粗圆的线（线宽是边长的 0.15，和 Logo
/// 一样）、不带底，Logo 的两种颜色渐变过去（左上角先掺一点白）
private struct StrokeMark: View {
    let size: CGFloat
    let from: Color
    let to: Color
    let path: (CGFloat) -> Path

    var body: some View {
        Canvas { ctx, s in
            let w = s.width
            let shade = Gradient(stops: [.init(color: from.mix(with: .white, by: 0.25), location: 0), .init(color: from, location: 0.35),
                                         .init(color: to, location: 1)])
            ctx.stroke(path(w), with: .linearGradient(shade, startPoint: CGPoint(x: 0.15 * w, y: 0), endPoint: CGPoint(x: 0.85 * w, y: w)),
                       style: StrokeStyle(lineWidth: w * 0.15, lineCap: .round, lineJoin: .round))
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// 三根高低不同的柱子，蓝渐变到紫
struct StatsMark: View {
    var size: CGFloat = 22

    var body: some View {
        StrokeMark(size: size, from: Brand.blue, to: Brand.violet) { w in
            func p(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: x * w, y: y * w) }
            var g = Path()
            g.move(to: p(0.20, 0.84)); g.addLine(to: p(0.20, 0.56))
            g.move(to: p(0.50, 0.84)); g.addLine(to: p(0.50, 0.16))
            g.move(to: p(0.80, 0.84)); g.addLine(to: p(0.80, 0.40))
            return g
        }
    }
}

/// 美元符号（一笔 S，上下各露出一小截竖线），琥珀渐变到玫红
struct CostMark: View {
    var size: CGFloat = 22

    var body: some View {
        StrokeMark(size: size, from: Brand.amber, to: Brand.rose) { w in
            func p(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: x * w, y: y * w) }
            var g = Path()
            g.move(to: p(0.71, 0.30))
            g.addCurve(to: p(0.50, 0.21), control1: p(0.68, 0.24), control2: p(0.60, 0.21))
            g.addCurve(to: p(0.29, 0.355), control1: p(0.39, 0.21), control2: p(0.29, 0.27))
            g.addCurve(to: p(0.50, 0.50), control1: p(0.29, 0.44), control2: p(0.40, 0.475))
            g.addCurve(to: p(0.71, 0.645), control1: p(0.60, 0.525), control2: p(0.71, 0.56))
            g.addCurve(to: p(0.50, 0.79), control1: p(0.71, 0.73), control2: p(0.61, 0.79))
            g.addCurve(to: p(0.29, 0.70), control1: p(0.40, 0.79), control2: p(0.32, 0.76))
            g.move(to: p(0.50, 0.08)); g.addLine(to: p(0.50, 0.21))
            g.move(to: p(0.50, 0.79)); g.addLine(to: p(0.50, 0.92))
            return g
        }
    }
}

/// 扣在一起的两个环，青绿渐变到紫
struct SharingMark: View {
    var size: CGFloat = 22

    var body: some View {
        StrokeMark(size: size, from: Brand.teal, to: Brand.violet) { w in
            var g = Path()
            g.addEllipse(in: CGRect(x: 0.10 * w, y: 0.27 * w, width: 0.46 * w, height: 0.46 * w))
            g.addEllipse(in: CGRect(x: 0.44 * w, y: 0.27 * w, width: 0.46 * w, height: 0.46 * w))
            return g
        }
    }
}

/// 中性的小胶囊：「只看」「Team 名」；color 给了的话前面一个小圆点（或者图标）是这个颜色
struct Tag: View {
    let text: String
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

/// 提示条（内容层，实的）：按严重程度染一层很淡的颜色，图标和文字，下面是按钮（解决问题的那个 .borderedProminent，其余 .bordered）
struct Banner<Actions: View>: View {
    let systemImage: String
    let tint: Color
    let title: String
    var message: String = ""
    let actions: Actions

    init(systemImage: String, tint: Color, title: String, message: String = "", @ViewBuilder actions: () -> Actions) {
        self.systemImage = systemImage
        self.tint = tint
        self.title = title
        self.message = message
        self.actions = actions()
    }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 14, style: .continuous)
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Image(systemName: systemImage)
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(tint)
                    .frame(width: 22)
                VStack(alignment: .leading, spacing: 3) {
                    Text(title).font(.callout.weight(.semibold)).fixedSize(horizontal: false, vertical: true)
                    if !message.isEmpty {
                        Text(message).font(.footnote).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer(minLength: 0)
            }
            HStack(spacing: 8) { actions }
                .controlSize(.small)
                .padding(.leading, 32)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(shape.fill(tint.opacity(0.09)))
        .overlay(shape.strokeBorder(tint.opacity(0.24), lineWidth: 0.5))
        .accessibilityElement(children: .contain)
    }
}

extension Banner where Actions == EmptyView {
    init(systemImage: String, tint: Color, title: String, message: String = "") {
        self.init(systemImage: systemImage, tint: tint, title: title, message: message) { EmptyView() }
    }
}

/// 卡片（实的色块）：图标、标题、内容
struct Card<Content: View>: View {
    let title: String
    let systemImage: String
    let content: Content

    init(_ title: String, systemImage: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.systemImage = systemImage
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label(title, systemImage: systemImage)
                .font(.headline)
            content
        }
        .padding(18)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Surface.tile))
    }
}

/// 状态点：接着、在线是实心绿；没有回应是橙圈；不在线、断了是灰圈；还在接是淡的
struct StatusDot: View {
    enum Kind { case on, busy, off, pending }
    let kind: Kind

    var body: some View {
        Group {
            switch kind {
            case .on: Circle().fill(.green)
            case .busy: Circle().strokeBorder(.orange, lineWidth: 1.5)
            case .off: Circle().strokeBorder(.secondary, lineWidth: 1.5)
            case .pending: Circle().fill(.quaternary)
            }
        }
        .frame(width: 8, height: 8)
        .accessibilityHidden(true)
    }
}

extension View {
    /// 主按钮 .glassProminent，其它 .glass（和 Mac App 一样：玻璃只给控件）
    @ViewBuilder
    func glassButton(prominent: Bool) -> some View {
        if prominent {
            self.buttonStyle(.glassProminent)
        } else {
            self.buttonStyle(.glass)
        }
    }

    /// 内容的版心：iPad、横屏时不铺满整个宽度
    func pageColumn() -> some View {
        frame(maxWidth: 640).frame(maxWidth: .infinity)
    }
}
