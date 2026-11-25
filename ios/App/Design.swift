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
}

/// 内容层的色块（不是玻璃）：深浅色下都比背景深一点 / 亮一点
enum Surface {
    static let tile = Color.primary.opacity(0.045)
    static let hairline = Color.primary.opacity(0.12)
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
