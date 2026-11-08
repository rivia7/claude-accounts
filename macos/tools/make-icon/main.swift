// 画 cca.app 的图标（1024 × 1024 PNG）：swiftc tools/make-icon/main.swift Sources/CcaApp/LogoGlyph.swift -o make-icon && ./make-icon out.png
//
// 只用 CoreGraphics（形状取 SwiftUI 的连续圆角矩形，和系统图标的轮廓一样）。按 Apple 的图标网格：
// 圆角方形边长 824、四周各留 100（0.0977 × 1024），圆角 185.4；所有内容都裁在圆角方形里，外面一个像素都不画
// （macOS 26 起，轮廓不对的图标会被缩小、关进灰色的底板里）。
// 图案是原创的：白底上一根粗圆的线分成三支，主干蓝色，三支各自渐变到玫红、青绿、琥珀（一个出口分给几个账号）。
import CoreGraphics
import Foundation
import ImageIO
import SwiftUI
import UniformTypeIdentifiers

let S: CGFloat = 1024
let inset: CGFloat = 100
let body = CGRect(x: inset, y: inset, width: S - 2 * inset, height: S - 2 * inset)
let radius: CGFloat = 185.4

let space = CGColorSpace(name: CGColorSpace.sRGB)!
guard let ctx = CGContext(data: nil, width: Int(S), height: Int(S), bitsPerComponent: 8, bytesPerRow: 0, space: space,
                          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
    fatalError("no context")
}
ctx.setAllowsAntialiasing(true)
ctx.setShouldAntialias(true)
ctx.interpolationQuality = .high

// CoreGraphics 的原点在左下角；下面的坐标都按“上为 y 小”来想，统一翻转一次
ctx.translateBy(x: 0, y: S)
ctx.scaleBy(x: 1, y: -1)

let squircle = RoundedRectangle(cornerRadius: radius, style: .continuous).path(in: body).cgPath

func gradient(_ stops: [(CGFloat, CGColor)]) -> CGGradient {
    CGGradient(colorsSpace: space, colors: stops.map { $0.1 } as CFArray, locations: stops.map { $0.0 })!
}

ctx.saveGState()
ctx.addPath(squircle)
ctx.clip()

// 底：干净的白，往下一点点灰（参考 Google 2025 年后的图标：一块干净的底上一个渐变的形，扁平、不加高光和阴影）
ctx.drawLinearGradient(gradient([(0, LogoGlyph.color(0xFFFFFF)), (1, LogoGlyph.color(0xEDF0F6))]),
                       start: CGPoint(x: 512, y: body.minY), end: CGPoint(x: 512, y: body.maxY), options: [])

// 字形（LogoGlyph.swift，和 App 里的 BrandMark 同一份）：居中，边长 592
LogoGlyph.draw(in: ctx, rect: CGRect(x: 512 - 296, y: 512 - 296, width: 592, height: 592))

// 内描边：浅色背景（Finder、白色的桌面）上也看得出边
ctx.addPath(squircle)
ctx.setStrokeColor(LogoGlyph.color(0x000000, alpha: 0.07))
ctx.setLineWidth(5)
ctx.strokePath()
ctx.restoreGState()

guard let image = ctx.makeImage() else { fatalError("no image") }
let out = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon_1024.png")
guard let dest = CGImageDestinationCreateWithURL(out as CFURL, UTType.png.identifier as CFString, 1, nil) else {
    fatalError("cannot write \(out.path)")
}
CGImageDestinationAddImage(dest, image, nil)
guard CGImageDestinationFinalize(dest) else { fatalError("cannot write \(out.path)") }
