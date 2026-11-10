import SwiftUI

// Liquid Glass 只用在控件层（按钮、浮着的提示）；侧栏、工具栏由系统画成玻璃。
// 内容层（额度、线路、分流图、提示条）是实的，用 Surface 的色块，玻璃上不再叠玻璃。

/// 分区标题：左边是标题，右边是这一节的状态和操作（操作放在它作用的内容旁边）。
struct SectionHeader<Accessory: View>: View {
    let title: String
    let accessory: Accessory

    init(_ title: String, @ViewBuilder accessory: () -> Accessory) {
        self.title = title
        self.accessory = accessory()
    }

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            Text(title)
                .font(.title3.weight(.semibold))
                .accessibilityAddTraits(.isHeader)
            Spacer(minLength: 8)
            accessory
        }
        .frame(minHeight: 28)
    }
}

// 按钮的样子只有几种（写进 macos/README.md「界面的规矩」）：
// - 一节标题上的操作（刷新用量、检测出口、显示细节）用 SectionAction；一行信息右边的操作（重新登录…、在控制台查看、复制）用
//   RowAction。两种一个样子：玻璃、图标加文字、常规大小，右边和标题上的按钮对齐；行里的按钮一样宽、左边也对齐。
// - 打开网页的（在账号自己的浏览器里）图标是 ↗（RowAction.web）。
// - 提示条里的按钮小一号（Banner 设的）：解决问题的那个 .borderedProminent，其余 .bordered。

/// 行里按钮的标签至少这么宽：同一节里上下几个按钮一样宽（最长的「在 claude.ai 查看」也放得下；更长的只会让它自己变宽）。
enum RowMetrics {
    static let actionLabelWidth: CGFloat = 124
}

/// 一节标题上的操作：玻璃按钮、图标加文字、常规大小。
struct SectionAction: View {
    let title: String
    let systemImage: String
    var prominent = false
    let action: () -> Void

    init(_ title: String, systemImage: String, prominent: Bool = false, action: @escaping () -> Void) {
        self.title = title
        self.systemImage = systemImage
        self.prominent = prominent
        self.action = action
    }

    var body: some View {
        Button(action: action) { Label(title, systemImage: systemImage) }
            .glassButton(prominent: prominent)
            .controlSize(.regular)
    }
}

extension View {
    /// 主按钮用 .glassProminent，其它 .glass。
    @ViewBuilder
    func glassButton(prominent: Bool) -> some View {
        if prominent {
            self.buttonStyle(.glassProminent)
        } else {
            self.buttonStyle(.glass)
        }
    }
}

/// 一行信息右边的操作：和 SectionAction 一个样子，标签有最小宽度（同一节里的几个一样宽）。
struct RowAction: View {
    let title: String
    let systemImage: String
    let action: () -> Void

    init(_ title: String, systemImage: String, action: @escaping () -> Void) {
        self.title = title
        self.systemImage = systemImage
        self.action = action
    }

    /// 在账号自己的浏览器里打开一个网页（图标是 ↗）
    static func web(_ title: String, action: @escaping () -> Void) -> RowAction {
        RowAction(title, systemImage: "arrow.up.right.square", action: action)
    }

    var body: some View {
        Button(action: action) {
            Label(title, systemImage: systemImage)
                .frame(minWidth: RowMetrics.actionLabelWidth)
        }
        .buttonStyle(.glass)
        .controlSize(.regular)
    }
}

/// 一行信息：左边图标和说明（太长会折行），中间可以放一个值，右边是这一行的操作（RowAction）。
/// 不缩进：左边和额度砖、右边和标题上的按钮对齐。
struct InfoRow<Value: View, Actions: View>: View {
    let title: Text
    let systemImage: String
    var tint: Color?
    let value: Value
    let actions: Actions

    init(_ title: Text, systemImage: String, tint: Color? = nil, @ViewBuilder value: () -> Value, @ViewBuilder actions: () -> Actions) {
        self.title = title
        self.systemImage = systemImage
        self.tint = tint
        self.value = value()
        self.actions = actions()
    }

    var body: some View {
        HStack(spacing: 10) {
            Label {
                title.foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            } icon: {
                Image(systemName: systemImage).foregroundStyle(tint.map { AnyShapeStyle($0) } ?? AnyShapeStyle(.secondary))
            }
            .labelStyle(FixedIconLabelStyle(iconWidth: 20, spacing: 8))
            Spacer(minLength: 8)
            value
            GlassRow { actions }
        }
        .font(.callout)
        .frame(minHeight: 28)
        .accessibilityElement(children: .contain)
    }
}

extension InfoRow where Value == EmptyView {
    init(_ title: Text, systemImage: String, tint: Color? = nil, @ViewBuilder actions: () -> Actions) {
        self.init(title, systemImage: systemImage, tint: tint, value: { EmptyView() }, actions: actions)
    }
}

/// 图标占固定的宽度、文字左边对齐（SF Symbols 各个宽度不一样，上下几行的文字会对不齐）。
struct FixedIconLabelStyle: LabelStyle {
    var iconWidth: CGFloat
    var spacing: CGFloat = 8

    func makeBody(configuration: Configuration) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: spacing) {
            configuration.icon.frame(width: iconWidth)
            configuration.title
        }
    }
}

/// 「显示细节」开关（账号页的网络出口、分流页共用一个设置）：样子和标题上的别的按钮一样。
struct DetailsToggle: View {
    @Environment(Guide.self) private var guide
    let help: String

    var body: some View {
        SectionAction(guide.showDetails ? "收起细节" : "显示细节", systemImage: "list.bullet.below.rectangle") {
            withAnimation(.snappy) { guide.toggleDetails() }
        }
        .help(help)
    }
}

/// 侧栏和账号行里的灰字。侧栏的列表给 .secondary 用系统的混色（vibrancy），深色外观下对比度只有约 3:1，小字发虚，
/// 清掉 backgroundMaterial 也没用（列表不认它）；这里直接用不混色的 55% 白，和内容区的 .secondary 一样，约 5.6:1。
/// 只在深色外观、窗口在前台时这样：窗口在后台时系统把整个侧栏调暗（名字也暗，灰字不能比名字亮），浅色外观、选中后底色变强的行也都照系统的。
struct PlainSecondary: ShapeStyle {
    /// 不换颜色时用系统的哪一级（侧栏的分组标题、版本号本来就淡一级）
    var system = HierarchicalShapeStyle.secondary

    func resolve(in environment: EnvironmentValues) -> AnyShapeStyle {
        environment.colorScheme == .dark && environment.controlActiveState != .inactive && environment.backgroundProminence != .increased
            ? AnyShapeStyle(Color.white.opacity(0.55)) : AnyShapeStyle(system)
    }
}

extension ShapeStyle where Self == PlainSecondary {
    static var plainSecondary: PlainSecondary { PlainSecondary() }
}

extension View {
    /// 菜单栏面板的文字用普通的颜色：深色外观下玻璃的混色（vibrancy）让灰字的对比度只有约 3:1，小字发虚，状态色也比内容区的艳；
    /// 不用它，灰字约 6:1，和内容区一样。浅色外观下这层混色反而让字更深，不动。
    func plainTextOnDarkGlass() -> some View {
        modifier(PlainTextOnDarkGlass())
    }
}

private struct PlainTextOnDarkGlass: ViewModifier {
    @Environment(\.colorScheme) private var colorScheme

    func body(content: Content) -> some View {
        content.transformEnvironment(\.backgroundMaterial) { if colorScheme == .dark { $0 = nil } }
    }
}

extension View {
    /// 详情页的版心：内容最宽 1400pt，两边各留 28pt，再宽就居中。顶上的提示条、账号页、分流页都用它，左右边对齐。
    func pageColumn() -> some View {
        frame(maxWidth: 1400, alignment: .leading)
            .padding(.horizontal, 28)
            .frame(maxWidth: .infinity)
    }
}

/// 几节内容（账号页的用量、网络出口）：每栏都能有 minColumnWidth 宽时左右并排（一样宽、顶端对齐），不够宽就上下排。
/// 用 Layout 而不用 ViewThatFits：换排法时还是同一份视图（不会建两份、不丢状态），也不用先量宽度再排一遍。
struct SectionColumns: Layout {
    var minColumnWidth: CGFloat = 500
    var spacing: CGFloat = 32

    /// 并排时每栏的宽度；不并排时为 nil。
    private func columnWidth(_ width: CGFloat?, count: Int) -> CGFloat? {
        guard let width, width.isFinite, count > 1 else { return nil }
        let w = (width - spacing * CGFloat(count - 1)) / CGFloat(count)
        return w >= minColumnWidth ? w : nil
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        if let col = columnWidth(proposal.width, count: subviews.count) {
            let heights = subviews.map { $0.sizeThatFits(ProposedViewSize(width: col, height: nil)).height }
            return CGSize(width: proposal.width ?? 0, height: heights.max() ?? 0)
        }
        let sizes = subviews.map { $0.sizeThatFits(ProposedViewSize(width: proposal.width, height: nil)) }
        let height = sizes.map(\.height).reduce(0, +) + spacing * CGFloat(max(sizes.count - 1, 0))
        return CGSize(width: proposal.width ?? sizes.map(\.width).max() ?? 0, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        if let col = columnWidth(bounds.width, count: subviews.count) {
            let p = ProposedViewSize(width: col, height: nil)
            for (i, s) in subviews.enumerated() {
                s.place(at: CGPoint(x: bounds.minX + CGFloat(i) * (col + spacing), y: bounds.minY), anchor: .topLeading, proposal: p)
            }
            return
        }
        let p = ProposedViewSize(width: bounds.width, height: nil)
        var y = bounds.minY
        for s in subviews {
            s.place(at: CGPoint(x: bounds.minX, y: y), anchor: .topLeading, proposal: p)
            y += s.sizeThatFits(p).height + spacing
        }
    }
}

/// 卡片（实的色块）：引导页、客户端更新页。图标占固定的宽度，几张卡片的标题左边对齐。
struct Card<Icon: View, Content: View>: View {
    let title: String
    let icon: Icon
    let content: Content

    init(_ title: String, @ViewBuilder icon: () -> Icon, @ViewBuilder content: () -> Content) {
        self.title = title
        self.icon = icon()
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label { Text(title) } icon: { icon }
                .labelStyle(FixedIconLabelStyle(iconWidth: 26, spacing: 10))
                .font(.title3.weight(.semibold))
            content
        }
        .padding(22)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Surface.tile))
    }
}

extension Card where Icon == Image {
    init(_ title: String, systemImage: String, @ViewBuilder content: () -> Content) {
        self.init(title, icon: { Image(systemName: systemImage) }, content: content)
    }
}

/// 一排相邻的玻璃按钮：放在一个 GlassEffectContainer 里一起渲染；容器的 spacing 比排列的间距小，彼此不会融在一起。
struct GlassRow<Content: View>: View {
    var spacing: CGFloat = 8
    let content: Content

    init(spacing: CGFloat = 8, @ViewBuilder content: () -> Content) {
        self.spacing = spacing
        self.content = content()
    }

    var body: some View {
        GlassEffectContainer(spacing: max(spacing / 2, 1)) {
            HStack(spacing: spacing) { content }
        }
    }
}

/// 提示条（内容层，实的）：按严重程度染一层很淡的颜色，左边图标和文字，右边按钮（.bordered / .borderedProminent）。
/// 文字那一栏有个最小宽度、排版时优先：按钮多的时候也不会被挤成一个字一行（那样提示条会高得把整个窗口撑出去）。
/// messageLines 给了就最多显示这么多行（多的省略），不给就全显示。
struct Banner<Actions: View>: View {
    let systemImage: String
    let tint: Color
    let title: String
    var message: String = ""
    var messageLines: Int?
    let actions: Actions

    init(systemImage: String, tint: Color, title: String, message: String = "", messageLines: Int? = nil,
         @ViewBuilder actions: () -> Actions) {
        self.systemImage = systemImage
        self.tint = tint
        self.title = title
        self.message = message
        self.messageLines = messageLines
        self.actions = actions()
    }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 12, style: .continuous)
        HStack(alignment: .center, spacing: 12) {
            Image(systemName: systemImage)
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(tint)
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.callout.weight(.semibold))
                    .fixedSize(horizontal: false, vertical: true)
                if !message.isEmpty {
                    Text(message)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(messageLines)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
            }
            .frame(minWidth: 180, alignment: .leading)
            .layoutPriority(1)
            Spacer(minLength: 8)
            HStack(spacing: 8) { actions }
                .controlSize(.small)
                .fixedSize()
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
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

/// 一叠提示条。
struct BannerStack<Content: View>: View {
    let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        VStack(spacing: 8) { content }
    }
}

/// 窗口底部浮着的提示（玻璃）：几秒后自己消失。
struct ToastView: View {
    let toast: Toast

    var body: some View {
        let (icon, color): (String, Color) = {
            switch toast.style {
            case .success: return ("checkmark.circle.fill", .green)
            case .warning: return ("exclamationmark.triangle.fill", .orange)
            case .info: return ("info.circle.fill", .accentColor)
            }
        }()
        HStack(spacing: 8) {
            Image(systemName: icon).foregroundStyle(color)
            Text(toast.text).font(.callout).lineLimit(3).textSelection(.enabled)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 9)
        .glassEffect(.regular, in: .capsule)
        .accessibilityElement(children: .combine)
    }
}

extension View {
    /// macOS 27 的标签页样式，26 上用分段控件。
    @ViewBuilder
    func toolPickerStyle() -> some View {
        if #available(macOS 27, *) {
            self.pickerStyle(.tabs)
        } else {
            self.pickerStyle(.segmented)
        }
    }
}
