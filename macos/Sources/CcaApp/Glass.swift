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

extension SectionHeader where Accessory == EmptyView {
    init(_ title: String) {
        self.init(title) { EmptyView() }
    }
}

/// 引导页的卡片（实的色块）。
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
        VStack(alignment: .leading, spacing: 14) {
            Label(title, systemImage: systemImage)
                .font(.title3.weight(.semibold))
            content
        }
        .padding(22)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Surface.tile))
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
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
            }
            Spacer(minLength: 8)
            actions.controlSize(.small)
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
