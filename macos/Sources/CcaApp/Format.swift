import AppKit
import SwiftUI

// 状态 → 语气 / 颜色，头像，品牌标记，账号的几行文字。文字本身在 CcaKit 的 Texts 里（可测试）。

enum Palette {
    /// 用量的颜色：平时是强调色，接近上限琥珀色，到了上限红色（magpie 的额度条也是强调色，满了换琥珀色）。
    static func usage(_ level: UsageLevel) -> Color {
        switch level {
        case .normal: return Theme.accent
        case .warning: return Theme.amber
        case .critical: return Theme.red
        }
    }

    static func usage(percent: Double) -> Color { usage(UsageLevel(percent: percent)) }

    static func slot(_ s: SlotState?) -> Tone {
        switch s {
        case .some(.ok): return .ok
        case .some(.drift): return .drift
        case .some(.closed): return .bad
        case .some(.empty): return .idle
        default: return .unknown
        }
    }

    static func login(_ s: LoginState) -> Tone {
        switch s {
        case .loggedIn: return .ok
        case .loggedOut: return .idle
        case .emailMismatch: return .warn
        case .api: return .info
        default: return .unknown
        }
    }

    static func health(_ r: HealthResult?) -> Tone {
        switch r {
        case .some(.ok), .some(.apiOk): return .ok
        case .some(.lowPurity), .some(.noScore): return .warn
        case .some(.restricted), .some(.noExit), .some(.portClosed), .some(.apiUnreachable): return .bad
        default: return .unknown
        }
    }

    static func leak(_ s: LeakState?) -> Tone {
        switch s {
        case .some(.ok): return .ok
        case .some(.leak): return .bad
        case .some(.failed): return .warn
        default: return .unknown
        }
    }
}

/// 品牌标记：App 图标的线稿（一个出口分出三条线），用正文的墨色，和 “cca” 字样一起放在工具栏、菜单栏面板的左上角。
struct BrandGlyph: View {
    var size: CGFloat = 18

    var body: some View {
        Canvas { ctx, s in
            let w = s.width, h = s.height
            let hub = CGPoint(x: w * 0.2, y: h * 0.5)
            let ends = [CGPoint(x: w * 0.86, y: h * 0.16), CGPoint(x: w * 0.9, y: h * 0.5), CGPoint(x: w * 0.86, y: h * 0.84)]
            let line = max(w * 0.085, 1.2)
            for p in ends {
                var path = Path()
                path.move(to: CGPoint(x: hub.x + w * 0.1, y: hub.y))
                let mid = hub.x + (p.x - hub.x) * 0.5
                path.addCurve(to: CGPoint(x: p.x - w * 0.08, y: p.y),
                              control1: CGPoint(x: mid, y: hub.y), control2: CGPoint(x: mid, y: p.y))
                ctx.stroke(path, with: .foreground, style: StrokeStyle(lineWidth: line, lineCap: .round))
                let r = w * 0.085
                ctx.fill(Path(ellipseIn: CGRect(x: p.x - r, y: p.y - r, width: r * 2, height: r * 2)), with: .foreground)
            }
            let hr = w * 0.12
            ctx.stroke(Path(ellipseIn: CGRect(x: hub.x - hr, y: hub.y - hr, width: hr * 2, height: hr * 2)), with: .foreground, lineWidth: line)
        }
        .foregroundStyle(Theme.fg)
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// 品牌标记加 “cca” 字样。
struct BrandLabel: View {
    var body: some View {
        HStack(spacing: 6) {
            BrandGlyph(size: 17)
            Text("cca").font(.system(size: 13.5, weight: .semibold)).foregroundStyle(Theme.fg)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("cca")
    }
}

/// 账号头像：圆角方块，和启动器图标右下角一样的标识（文字 + 颜色）；API 账号是钥匙。
struct BadgeAvatar: View {
    let badge: Badge?
    var isAPI = false
    var size: CGFloat = 28

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
        ZStack {
            shape.fill(badge == nil ? Theme.pill : Color(badge: badge))
            if let b = badge, !b.label.isEmpty {
                Text(b.label)
                    .font(.system(size: size * 0.46, weight: .semibold, design: .rounded))
                    .foregroundStyle(.white)
                    .minimumScaleFactor(0.5)
                    .lineLimit(1)
                    .padding(size * 0.1)
            } else {
                Image(systemName: isAPI ? "key.horizontal.fill" : "person.fill")
                    .font(.system(size: size * 0.42, weight: .semibold))
                    .foregroundStyle(Theme.fg2)
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// 头像右下角的绿点：这个账号的桌面 App 开着。
struct RunningDot: View {
    var size: CGFloat = 9
    var ring: Color = Theme.card

    var body: some View {
        Circle().fill(Theme.green)
            .frame(width: size, height: size)
            .overlay(Circle().stroke(ring, lineWidth: max(size * 0.2, 1.5)))
    }
}

extension Account {
    /// 这个账号的颜色（启动器角标的颜色）：分流图里它的线、名字前的点；没有角标的（第三方 API 账号）是灰色。
    var lineColor: Color { badge == nil ? Color(nsColor: .systemGray) : Color(badge: badge) }

    /// 名字下面那行：登录状态、套餐（API 账号：接口地址）。
    var statusLine: String {
        if isAPI {
            return ["第三方 API", api?.host].compactMap { nonEmpty($0) }.joined(separator: " · ")
        }
        var parts = [Texts.login(login, loginEmail: login == .emailMismatch ? loginEmail : nil)]
        if let p = nonEmpty(plan) { parts.append(p) }
        return parts.joined(separator: " · ")
    }

    /// 一行里的简称：别名，没有别名时是邮箱 @ 前面那段。
    var shortName: String {
        if let a = nonEmpty(alias) { return a }
        let n = displayName
        if !isAPI, let at = n.firstIndex(of: "@") { return String(n[..<at]) }
        return n
    }
}
