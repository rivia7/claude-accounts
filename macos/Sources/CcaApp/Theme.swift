import AppKit
import SwiftUI

// 设计语言参考 magpie（github.com/yetone/magpie）：一个安静的底色，白色的列表卡片和细分隔线，黑白为主；
// 颜色只给“哪个账号”（账号色）和“什么状态”（绿 / 琥珀 / 红，失败画成叉）。颜色值取自 magpie 的 app.css（浅色、深色两套）。
// 窗口的工具栏仍然是系统的（Liquid Glass），内容层全是实的。
//
// 视图里的局部状态（悬停）用 `private let x = State(initialValue: …)`：只有命令行工具时 `@State` 宏编不过，
// 但 State 类型本身照样是 SwiftUI 的动态属性（读写 `x.wrappedValue`）。

extension NSColor {
    convenience init(hex: UInt32, alpha: CGFloat = 1) {
        self.init(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                  blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
    }
}

extension Color {
    /// 浅色、深色外观各一个值。
    static func adaptive(_ light: UInt32, _ dark: UInt32, lightAlpha: CGFloat = 1, darkAlpha: CGFloat = 1) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
                ? NSColor(hex: dark, alpha: darkAlpha) : NSColor(hex: light, alpha: lightAlpha)
        })
    }

    /// `#RRGGBB`（账号角标的颜色）；格式不对时用灰色。
    init(badge: Badge?) {
        if let (r, g, b) = badge?.rgb {
            self = Color(red: r, green: g, blue: b)
        } else {
            self = Color(nsColor: .systemGray)
        }
    }
}

enum Theme {
    static let bg = Color.adaptive(0xF4F4F6, 0x1A1A1E)
    static let card = Color.adaptive(0xFFFFFF, 0x232327)
    /// 展开的详情、卡片里的次一级底色
    static let card2 = Color.adaptive(0xF9F9FB, 0x1F1F23)
    static let pill = Color.adaptive(0xF1F1F4, 0x2D2D33)
    static let pillHover = Color.adaptive(0xE9E9EE, 0x36363D)
    static let segTrack = Color.adaptive(0xE6E6EB, 0xFFFFFF, darkAlpha: 0.035)
    static let segThumb = Color.adaptive(0xFFFFFF, 0xFFFFFF, darkAlpha: 0.15)
    static let line = Color.adaptive(0xE3E3E8, 0x333339)
    static let line2 = Color.adaptive(0xECECF0, 0x2C2C32)
    static let fg = Color.adaptive(0x1C1C21, 0xEDEDF1)
    static let fg2 = Color.adaptive(0x4B4B53, 0xB9B9C1)
    static let muted = Color.adaptive(0x85858D, 0x8B8B94)
    static let faint = Color.adaptive(0xB9B9C2, 0x56565F)
    /// 控件上安静的字（文字按钮、没选中的页签）：比 muted 深，对比度够 4.5:1
    static let ctl = Color.adaptive(0x63636B, 0xA1A1AA)
    static let accent = Color.adaptive(0x4F46E5, 0xA5B4FC)
    static let accentSoft = Color.adaptive(0xEEEDFD, 0x2B2C47)
    static let accentFg = Color.adaptive(0xFFFFFF, 0x14142A)
    static let green = Color.adaptive(0x189A58, 0x6FD99B)
    static let greenSoft = Color.adaptive(0xE4F6EC, 0x1E3A2B)
    static let red = Color.adaptive(0xD64545, 0xF28B82)
    static let redSoft = Color.adaptive(0xFBE9E9, 0x45272A)
    static let amber = Color.adaptive(0xB45309, 0xF2B544)
    static let amberSoft = Color.adaptive(0xFBF0DF, 0x3F3220)
    /// 选错节点（偏离了应有的样子）
    static let drift = Color.adaptive(0xCE2C31, 0xFF9592)
}

/// 状态的几种语气。
enum Tone {
    case ok, warn, bad, drift, idle, info, unknown

    var color: Color {
        switch self {
        case .ok: return Theme.green
        case .warn: return Theme.amber
        case .bad: return Theme.red
        case .drift: return Theme.drift
        case .idle: return Theme.faint
        case .info: return Theme.accent
        case .unknown: return Theme.muted
        }
    }

    var soft: Color {
        switch self {
        case .ok: return Theme.greenSoft
        case .warn: return Theme.amberSoft
        case .bad, .drift: return Theme.redSoft
        case .info: return Theme.accentSoft
        case .idle, .unknown: return Theme.pill
        }
    }
}

/// 状态点：失败的画成叉（只看形状也分得出，红绿色弱也认得），空闲的是空心圈。
struct StatusDot: View {
    let tone: Tone
    var size: CGFloat = 7

    var body: some View {
        ZStack {
            switch tone {
            case .bad, .drift:
                Image(systemName: "xmark")
                    .font(.system(size: size * 1.1, weight: .black))
                    .foregroundStyle(tone.color)
            case .idle:
                Circle().strokeBorder(Theme.faint, lineWidth: 1.2)
            default:
                Circle().fill(tone.color)
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

// MARK: 卡片、分隔线、小标题

extension View {
    /// 白色的列表卡片：圆角、一像素的边。
    func card(radius: CGFloat = 12, fill: Color = Theme.card, border: Color = Theme.line) -> some View {
        let shape = RoundedRectangle(cornerRadius: radius, style: .continuous)
        return self
            .background(fill, in: shape)
            .clipShape(shape)
            .overlay(shape.strokeBorder(border, lineWidth: 1))
    }
}

/// 列表里两行之间的细线。
struct Hairline: View {
    var body: some View {
        Rectangle().fill(Theme.line2).frame(height: 1)
    }
}

/// 分区的小标题（灰色、半粗），右边放这一节的操作。
struct Caption<Trailing: View>: View {
    let title: String
    let trailing: Trailing

    init(_ title: String, @ViewBuilder trailing: () -> Trailing) {
        self.title = title
        self.trailing = trailing()
    }

    var body: some View {
        HStack(alignment: .center, spacing: 8) {
            Text(title)
                .font(.system(size: 12.5, weight: .semibold))
                .foregroundStyle(Theme.muted)
                .accessibilityAddTraits(.isHeader)
            Spacer(minLength: 8)
            trailing
        }
        .frame(minHeight: 26)
        .padding(.horizontal, 4)
    }
}

extension Caption where Trailing == EmptyView {
    init(_ title: String) {
        self.init(title) { EmptyView() }
    }
}

// MARK: 小部件

/// 一根额度条：4 pt 高，底槽是 pill 色。
struct QuotaBar: View {
    var fraction: Double
    var color: Color
    var height: CGFloat = 4

    var body: some View {
        GeometryReader { g in
            ZStack(alignment: .leading) {
                Capsule().fill(Theme.pill)
                if fraction > 0 {
                    Capsule().fill(color).frame(width: max(g.size.width * min(fraction, 1), height))
                }
            }
        }
        .frame(height: height)
        .accessibilityHidden(true)
    }
}

/// 灰底的小块（节点名这类“值”）：圆角 6，字 12.5、半粗；`edge` 有颜色时加一圈细边（选错节点、连不上）。
struct FieldPill: View {
    let text: String
    var symbol: String?
    var edge: Color?

    var body: some View {
        HStack(spacing: 4) {
            Text(text).lineLimit(1)
            if let symbol {
                Image(systemName: symbol).font(.system(size: 9, weight: .semibold)).foregroundStyle(Theme.muted)
            }
        }
        .font(.system(size: 12.5, weight: .medium).monospacedDigit())
        .foregroundStyle(Theme.fg)
        .padding(.horizontal, 8)
        .frame(height: 24)
        .background(Theme.pill, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
        .overlay {
            if let edge {
                RoundedRectangle(cornerRadius: 6, style: .continuous).strokeBorder(edge.opacity(0.45), lineWidth: 1)
            }
        }
    }
}

/// 等宽字的小块（槽位名 CCA-1 这类代号）。
struct CodePill: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 11, design: .monospaced))
            .foregroundStyle(Theme.fg2)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Theme.pill, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
    }
}

/// 状态标签：点 + 文字（文字用 fg2，颜色只在点上）。
struct StatusLabel: View {
    let tone: Tone
    let text: String

    var body: some View {
        HStack(spacing: 5) {
            StatusDot(tone: tone, size: 7)
            Text(text).foregroundStyle(Theme.fg2)
        }
        .font(.system(size: 12))
        .lineLimit(1)
        .accessibilityElement(children: .combine)
    }
}

/// 提示条：按语气染一层很淡的底色；左边图标和文字，右边按钮。
struct Notice<Actions: View>: View {
    let tone: Tone
    let systemImage: String
    let title: String
    var message: String = ""
    let actions: Actions

    init(_ tone: Tone, systemImage: String, title: String, message: String = "", @ViewBuilder actions: () -> Actions) {
        self.tone = tone
        self.systemImage = systemImage
        self.title = title
        self.message = message
        self.actions = actions()
    }

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            Image(systemName: systemImage)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(tone.color)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundStyle(Theme.fg)
                    .fixedSize(horizontal: false, vertical: true)
                if !message.isEmpty {
                    Text(message)
                        .font(.system(size: 12))
                        .foregroundStyle(Theme.fg2)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
            }
            Spacer(minLength: 8)
            actions
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(tone.soft, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .accessibilityElement(children: .contain)
    }
}

extension Notice where Actions == EmptyView {
    init(_ tone: Tone, systemImage: String, title: String, message: String = "") {
        self.init(tone, systemImage: systemImage, title: title, message: message) { EmptyView() }
    }
}

// MARK: 按钮

/// 按钮的几种样子（magpie 的 .text / .text.action / .text.primary / .icon）。悬停时换底色。
struct MagpieButtonStyle: ButtonStyle {
    enum Kind { case text, action, primary, icon }
    var kind: Kind
    var large = false

    func makeBody(configuration: Configuration) -> some View {
        ButtonChrome(kind: kind, large: large, configuration: configuration)
    }
}

extension ButtonStyle where Self == MagpieButtonStyle {
    /// 文字按钮：平时灰，悬停时有底色。
    static var quiet: MagpieButtonStyle { MagpieButtonStyle(kind: .text) }
    /// 白底细边的按钮。
    static var action: MagpieButtonStyle { MagpieButtonStyle(kind: .action) }
    /// 主按钮：强调色的底。
    static var primary: MagpieButtonStyle { MagpieButtonStyle(kind: .primary) }
    /// 只有图标的方块按钮。
    static var icon: MagpieButtonStyle { MagpieButtonStyle(kind: .icon) }
}

private struct ButtonChrome: View {
    let kind: MagpieButtonStyle.Kind
    let large: Bool
    let configuration: ButtonStyleConfiguration
    @Environment(\.isEnabled) private var enabled
    private let hover = State(initialValue: false)

    var body: some View {
        let h = hover.wrappedValue && enabled
        let pressed = configuration.isPressed && enabled
        let shape = RoundedRectangle(cornerRadius: kind == .text ? 6 : 7, style: .continuous)
        configuration.label
            .font(.system(size: large ? 13 : 12.5, weight: kind == .primary ? .semibold : (kind == .action ? .medium : .regular)))
            .labelStyle(GapLabelStyle())
            .foregroundStyle(foreground(h))
            .padding(.horizontal, kind == .icon ? 0 : (kind == .text ? 7 : (large ? 13 : 10)))
            .frame(width: kind == .icon ? 28 : nil, height: kind == .text ? 24 : (large ? 30 : 26))
            .background(background(h, pressed), in: shape)
            .overlay {
                if kind == .action {
                    shape.strokeBorder(h ? Theme.muted.opacity(0.6) : Theme.line, lineWidth: 1)
                }
            }
            .shadow(color: kind == .action ? .black.opacity(0.04) : .clear, radius: 0.5, y: 1)
            .contentShape(shape)
            .opacity(enabled ? 1 : 0.45)
            .onHover { hover.wrappedValue = $0 }
            .animation(.easeOut(duration: 0.12), value: h)
    }

    private func foreground(_ h: Bool) -> Color {
        switch kind {
        case .text: return h ? Theme.fg : Theme.ctl
        case .action: return h ? Theme.fg : Theme.fg2
        case .primary: return Theme.accentFg
        case .icon: return h ? Theme.fg : Theme.muted
        }
    }

    private func background(_ h: Bool, _ pressed: Bool) -> Color {
        switch kind {
        case .text, .icon: return pressed ? Theme.pillHover : (h ? Theme.pill : .clear)
        case .action: return pressed ? Theme.pill : (h ? Theme.card2 : Theme.card)
        case .primary: return Theme.accent.opacity(pressed ? 0.8 : (h ? 0.9 : 1))
        }
    }
}

/// 图标和文字之间留 5 pt（系统的 Label 间距太大）。
struct GapLabelStyle: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 5) {
            configuration.icon.font(.system(size: 11.5, weight: .medium))
            configuration.title
        }
    }
}

/// 扁平的分段控件（magpie 的页签）：灰色的槽，选中的那个是白色的块。
struct SegControl<Value: Hashable>: View {
    let options: [(Value, String)]
    let selection: Binding<Value>

    var body: some View {
        HStack(spacing: 0) {
            ForEach(options.indices, id: \.self) { i in
                let value = options[i].0
                let on = selection.wrappedValue == value
                Button {
                    selection.wrappedValue = value
                } label: {
                    Text(options[i].1)
                        .font(.system(size: 12.5, weight: on ? .medium : .regular))
                        .foregroundStyle(on ? Theme.fg : Theme.ctl)
                        .padding(.horizontal, 11)
                        .frame(height: 24)
                        .background {
                            if on {
                                RoundedRectangle(cornerRadius: 6, style: .continuous)
                                    .fill(Theme.segThumb)
                                    .shadow(color: .black.opacity(0.12), radius: 1, y: 1)
                            }
                        }
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(2)
        .background(Theme.segTrack, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }
}

/// 一行列表的悬停底色（展开详情、点了能跳转的行）。
struct HoverRow<Content: View>: View {
    let content: Content
    var active = false
    private let hover = State(initialValue: false)

    init(active: Bool = false, @ViewBuilder content: () -> Content) {
        self.active = active
        self.content = content()
    }

    var body: some View {
        content
            .background(hover.wrappedValue || active ? Theme.card2 : Color.clear)
            .onHover { hover.wrappedValue = $0 }
    }
}
