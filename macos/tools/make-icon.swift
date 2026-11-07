// 画 cca.app 的图标（1024 × 1024 PNG）：swiftc tools/make-icon.swift -o make-icon && ./make-icon out.png
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

func rgb(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255, alpha: a)
}

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
ctx.drawLinearGradient(gradient([(0, rgb(0xFFFFFF)), (1, rgb(0xEDF0F6))]),
                       start: CGPoint(x: 512, y: body.minY), end: CGPoint(x: 512, y: body.maxY), options: [])

// 字形：粗圆的一根线分成三支（一个出口分给几个账号）。主干蓝色，每一支从蓝渐变到自己的颜色（玫红、青绿、琥珀），
// 颜色互相流过去，不是一块一块。和 App 里的 BrandMark 同一个形
let box = CGRect(x: 512 - 296, y: 512 - 296, width: 592, height: 592)
func at(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: box.minX + x * box.width, y: box.minY + y * box.height) }
let root = at(0.12, 0.5), fork = at(0.38, 0.5)
let blue: UInt32 = 0x3B7BFF
// 每一支的渐变：分开之前都是蓝（三支在接头处叠着，颜色一样才看不出叠），分开以后才流向自己的颜色；
// 中间放饱和的颜色（蓝直接到琥珀会经过发灰的中间色），最下面那支走完整条 Logo 渐变（蓝 → 紫 → 玫红 → 琥珀）
let branches: [(CGPoint, [(CGFloat, UInt32)])] = [
    (at(0.82, 0.17), [(0, blue), (0.3, blue), (0.65, 0x7C5CFF), (1, 0xEC4C8C)]),
    (at(0.86, 0.5), [(0, blue), (0.3, blue), (0.65, 0x22A3E6), (1, 0x14B8A6)]),
    (at(0.82, 0.83), [(0, blue), (0.28, blue), (0.52, 0x7C5CFF), (0.76, 0xEC4C8C), (1, 0xF5A524)]),
]
let weight = box.width * 0.15
for i in [1, 0, 2] {
    let (tip, stops) = branches[i]
    let path = CGMutablePath()
    path.move(to: fork)
    let k = fork.x + (tip.x - fork.x) * 0.5
    path.addCurve(to: tip, control1: CGPoint(x: k, y: fork.y), control2: CGPoint(x: k, y: tip.y))
    ctx.saveGState()
    ctx.addPath(path.copy(strokingWithWidth: weight, lineCap: .round, lineJoin: .round, miterLimit: 10))
    ctx.clip()
    ctx.drawLinearGradient(gradient(stops.map { ($0.0, rgb($0.1)) }), start: fork, end: tip,
                           options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
    ctx.restoreGState()
}
// 主干（接头处和三支一样是蓝色，盖上去看不出接缝）
do {
    let stem = CGMutablePath()
    stem.move(to: root)
    stem.addLine(to: fork)
    ctx.addPath(stem.copy(strokingWithWidth: weight, lineCap: .round, lineJoin: .round, miterLimit: 10))
    ctx.setFillColor(rgb(blue))
    ctx.fillPath()
}

// 内描边：浅色背景（Finder、白色的桌面）上也看得出边
ctx.addPath(squircle)
ctx.setStrokeColor(rgb(0x000000, 0.07))
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
