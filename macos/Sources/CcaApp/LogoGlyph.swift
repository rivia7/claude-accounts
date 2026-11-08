import CoreGraphics

/// Logo 的字形：一根粗圆的线分成三支（一个出口分给几个账号）。主干蓝色，每一支从蓝渐变到自己的颜色（玫红、青绿、琥珀），
/// 颜色互相流过去，不是一块一块。App 图标（tools/make-icon/main.swift：build.sh 把这个文件一起编进去）和 App 里的
/// BrandMark 都用 draw 画，Brand 的颜色也取这里的：形状、颜色只写在这一处。
enum LogoGlyph {
    static let blue: UInt32 = 0x3B7BFF
    static let violet: UInt32 = 0x7C5CFF
    static let rose: UInt32 = 0xEC4C8C
    static let teal: UInt32 = 0x14B8A6
    static let amber: UInt32 = 0xF5A524
    /// 中间那支从蓝到青绿路过的饱和色
    private static let sky: UInt32 = 0x22A3E6

    /// 每一支的末端（单位正方形里，上为 y 小）和渐变（位置, 颜色），按画的顺序：中间那支先画。
    /// 分开之前都是蓝（三支在接头处叠着，颜色一样才看不出叠；0 到第一个位置之间用第一个颜色），分开以后才流向自己的颜色；
    /// 中间放饱和的颜色（蓝直接到琥珀会经过发灰的中间色），最下面那支走完整条 Logo 渐变（蓝 → 紫 → 玫红 → 琥珀）
    private static let branches: [(tip: CGPoint, stops: [(CGFloat, UInt32)])] = [
        (CGPoint(x: 0.86, y: 0.5), [(0.3, blue), (0.65, sky), (1, teal)]),
        (CGPoint(x: 0.82, y: 0.17), [(0.3, blue), (0.65, violet), (1, rose)]),
        (CGPoint(x: 0.82, y: 0.83), [(0.28, blue), (0.52, violet), (0.76, rose), (1, amber)]),
    ]

    /// 在 r 里画。c 的坐标要“上为 y 小”（SwiftUI Canvas 的 withCGContext 就是；CoreGraphics 的位图先翻转一次）。
    static func draw(in c: CGContext, rect r: CGRect) {
        func at(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: r.minX + x * r.width, y: r.minY + y * r.height) }
        let weight = r.width * 0.15, root = at(0.12, 0.5), fork = at(0.38, 0.5)
        let space = CGColorSpace(name: CGColorSpace.sRGB)!
        for b in branches {
            let tip = at(b.tip.x, b.tip.y), k = fork.x + (tip.x - fork.x) * 0.5
            let path = CGMutablePath()
            path.move(to: fork)
            path.addCurve(to: tip, control1: CGPoint(x: k, y: fork.y), control2: CGPoint(x: k, y: tip.y))
            guard let g = CGGradient(colorsSpace: space, colors: b.stops.map { color($0.1) } as CFArray,
                                     locations: b.stops.map { $0.0 }) else { continue }
            c.saveGState()
            c.addPath(path.copy(strokingWithWidth: weight, lineCap: .round, lineJoin: .round, miterLimit: 10))
            c.clip()
            c.drawLinearGradient(g, start: fork, end: tip, options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
            c.restoreGState()
        }
        // 主干（接头处和三支一样是蓝色，盖上去看不出接缝）
        let stem = CGMutablePath()
        stem.move(to: root)
        stem.addLine(to: fork)
        c.addPath(stem.copy(strokingWithWidth: weight, lineCap: .round, lineJoin: .round, miterLimit: 10))
        c.setFillColor(color(blue))
        c.fillPath()
    }

    static func color(_ hex: UInt32, alpha: CGFloat = 1) -> CGColor {
        CGColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
    }
}
