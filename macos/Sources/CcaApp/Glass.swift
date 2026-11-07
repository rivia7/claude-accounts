import SwiftUI

// Liquid Glass 只用在控件层（按钮、状态胶囊、横幅、浮动操作条）；内容卡片用普通的背景。

/// 内容卡片（不是玻璃）。
struct Card<Accessory: View, Content: View>: View {
    let title: String
    let systemImage: String
    let accessory: Accessory
    let content: Content

    init(_ title: String, systemImage: String, @ViewBuilder accessory: () -> Accessory, @ViewBuilder content: () -> Content) {
        self.title = title
        self.systemImage = systemImage
        self.accessory = accessory()
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Label(title, systemImage: systemImage)
                    .font(.headline)
                Spacer(minLength: 8)
                accessory
            }
            content
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(.background.secondary))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(.separator.opacity(0.6), lineWidth: 0.5))
    }
}

extension Card where Accessory == EmptyView {
    init(_ title: String, systemImage: String, @ViewBuilder content: () -> Content) {
        self.init(title, systemImage: systemImage, accessory: { EmptyView() }, content: content)
    }
}

/// 状态胶囊（玻璃）：文字是 primary（对比度够），状态的颜色只在前面的圆点 / 图标上；读屏软件读出完整的文字。
/// 一排相邻的胶囊放进 GlassEffectContainer（见 GlassRow）。
struct GlassPill: View {
    let text: String
    let color: Color
    var systemImage: String?

    var body: some View {
        HStack(spacing: 5) {
            if let systemImage {
                Image(systemName: systemImage).imageScale(.small).foregroundStyle(color)
            } else {
                Circle().fill(color).frame(width: 7, height: 7)
            }
            Text(text).foregroundStyle(.primary)
        }
        .font(.caption.weight(.semibold))
        .padding(.horizontal, 9)
        .padding(.vertical, 4)
        .glassEffect(.regular, in: .capsule)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(text)
    }
}

/// 一排相邻的玻璃（胶囊、按钮）：放在一个 GlassEffectContainer 里一起渲染；容器的 spacing 比排列的间距小，彼此不会融在一起。
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

/// 横幅：染色的玻璃，左边图标和文字，右边按钮（按钮用 .bordered / .borderedProminent：玻璃上不再叠玻璃）。
/// 几条横幅叠在一起时放进 GlassBannerStack。
struct GlassBanner<Actions: View>: View {
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
        HStack(alignment: .center, spacing: 12) {
            Image(systemName: systemImage)
                .font(.title3)
                .foregroundStyle(tint)
                .frame(width: 24)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.callout.weight(.semibold))
                if !message.isEmpty {
                    Text(message).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
            }
            Spacer(minLength: 8)
            actions
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .glassEffect(.regular.tint(tint.opacity(0.16)), in: .rect(cornerRadius: 14))
    }
}

/// 一叠横幅：一个 GlassEffectContainer（spacing 比排列的间距小，彼此不融在一起）。
struct GlassBannerStack<Content: View>: View {
    let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        GlassEffectContainer(spacing: 4) {
            VStack(spacing: 10) { content }
        }
    }
}

extension GlassBanner where Actions == EmptyView {
    init(systemImage: String, tint: Color, title: String, message: String = "") {
        self.init(systemImage: systemImage, tint: tint, title: title, message: message) { EmptyView() }
    }
}

/// 窗口顶部（或底部）浮着的提示：几秒后自己消失。
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
