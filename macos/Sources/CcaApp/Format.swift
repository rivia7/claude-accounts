import AppKit
import SwiftUI

// 颜色和小部件：用量的颜色档、状态颜色、头像。文字本身在 CcaKit 的 Texts 里（可测试）。

extension Color {
    /// `#RRGGBB`；格式不对时用灰色。
    init(badge: Badge?) {
        if let (r, g, b) = badge?.rgb {
            self = Color(red: r, green: g, blue: b)
        } else {
            self = Color(nsColor: .systemGray)
        }
    }
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

    static func login(_ s: LoginState) -> Color {
        switch s {
        case .loggedIn: return .green
        case .loggedOut: return .gray
        case .emailMismatch: return .orange
        case .api: return .blue
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

/// 账号头像：和启动器图标右下角一样的标识（文字 + 颜色）；API 账号是钥匙。
struct BadgeAvatar: View {
    let badge: Badge?
    var isAPI = false
    var size: CGFloat = 28

    var body: some View {
        ZStack {
            Circle().fill(badge == nil ? Color(nsColor: .systemGray).opacity(0.55) : Color(badge: badge))
            Circle().fill(LinearGradient(colors: [.white.opacity(0.28), .clear], startPoint: .top, endPoint: .center))
            if let b = badge, !b.label.isEmpty {
                Text(b.label)
                    .font(.system(size: size * 0.46, weight: .semibold, design: .rounded))
                    .foregroundStyle(.white)
                    .minimumScaleFactor(0.5)
                    .lineLimit(1)
                    .padding(size * 0.08)
            } else {
                Image(systemName: isAPI ? "key.horizontal.fill" : "person.fill")
                    .font(.system(size: size * 0.42, weight: .semibold))
                    .foregroundStyle(.white)
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
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
                Capsule().fill(.quaternary)
                if fraction > 0 {
                    Capsule()
                        .fill(dim ? AnyShapeStyle(.secondary) : AnyShapeStyle(color.gradient))
                        .frame(width: max(g.size.width * min(fraction, 1), height))
                }
            }
        }
        .frame(height: height)
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
        .padding(.horizontal, 7)
        .padding(.vertical, 2)
        .background(Capsule().fill(.quaternary.opacity(0.7)))
        .accessibilityElement(children: .combine)
    }
}

/// 登录状态的小圆点。
struct LoginDot: View {
    let state: LoginState

    var body: some View {
        Circle().fill(Palette.login(state)).frame(width: 7, height: 7)
            .accessibilityLabel(Texts.login(state, loginEmail: nil))
    }
}

extension Account {
    /// 侧栏第二行：套餐 · 节点；API 账号：接口 · 直连 / 节点。
    var subtitle: String {
        if isAPI {
            let host = api?.host ?? "第三方 API"
            return host + " · " + (node ?? "直连")
        }
        var parts: [String] = []
        if let p = nonEmpty(plan) { parts.append(p) }
        if login != .loggedIn { parts.append(Texts.login(login, loginEmail: nil)) }
        if let n = node { parts.append(n) }
        return parts.joined(separator: " · ")
    }
}
