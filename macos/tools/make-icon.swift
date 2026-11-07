// 画 cca.app 的图标（1024 × 1024 PNG）：swiftc tools/make-icon.swift -o make-icon && ./make-icon out.png
//
// 只用 CoreGraphics（形状取 SwiftUI 的连续圆角矩形，和系统图标的轮廓一样）。按 Apple 的图标网格：
// 圆角方形边长 824、四周各留 100（0.0977 × 1024），圆角 185.4；所有内容都裁在圆角方形里，外面一个像素都不画
// （macOS 26 起，轮廓不对的图标会被缩小、关进灰色的底板里）。
// 图案是原创的：深色渐变底上，一个出口节点分出几条线连到不同颜色的节点（账号经各自的节点出去）。
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

// 底：深靛蓝到夜蓝的竖向渐变，右下角一团青色的光
ctx.setFillColor(rgb(0x141B52))
ctx.fill(body)
ctx.drawLinearGradient(gradient([(0, rgb(0x2D3A9E)), (0.55, rgb(0x1A2266)), (1, rgb(0x0C1033))]),
                       start: CGPoint(x: body.minX, y: body.minY), end: CGPoint(x: body.maxX * 0.7, y: body.maxY),
                       options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
ctx.drawRadialGradient(gradient([(0, rgb(0x19C2B0, 0.55)), (1, rgb(0x19C2B0, 0))]),
                       startCenter: CGPoint(x: 800, y: 820), startRadius: 0, endCenter: CGPoint(x: 800, y: 820), endRadius: 520, options: [])
ctx.drawRadialGradient(gradient([(0, rgb(0x7B8CFF, 0.35)), (1, rgb(0x7B8CFF, 0))]),
                       startCenter: CGPoint(x: 240, y: 230), startRadius: 0, endCenter: CGPoint(x: 240, y: 230), endRadius: 460, options: [])

// 淡淡的点阵（网络的感觉）
ctx.setFillColor(rgb(0xFFFFFF, 0.055))
var y = body.minY + 40
while y < body.maxY {
    var x = body.minX + 40
    while x < body.maxX {
        ctx.fillEllipse(in: CGRect(x: x - 3.5, y: y - 3.5, width: 7, height: 7))
        x += 62
    }
    y += 62
}

// 出口节点和四个目标节点
let hub = CGPoint(x: 318, y: 512)
let targets: [(CGPoint, UInt32)] = [
    (CGPoint(x: 716, y: 292), 0x0A84FF),
    (CGPoint(x: 760, y: 450), 0x30A46C),
    (CGPoint(x: 760, y: 608), 0xD6409F),
    (CGPoint(x: 716, y: 760), 0xF5A524),
]

// 线：从出口节点出发的平滑曲线，先画一层柔和的光，再画实线（颜色从白渐变到节点色）
for (p, color) in targets {
    let path = CGMutablePath()
    path.move(to: hub)
    path.addCurve(to: p, control1: CGPoint(x: hub.x + 210, y: hub.y), control2: CGPoint(x: p.x - 230, y: p.y))
    ctx.saveGState()
    ctx.addPath(path)
    ctx.setLineWidth(52)
    ctx.setLineCap(.round)
    ctx.setStrokeColor(rgb(color, 0.16))
    ctx.strokePath()
    ctx.restoreGState()

    ctx.saveGState()
    ctx.addPath(path)
    ctx.setLineWidth(24)
    ctx.setLineCap(.round)
    ctx.replacePathWithStrokedPath()
    ctx.clip()
    ctx.drawLinearGradient(gradient([(0, rgb(0xFFFFFF, 0.95)), (0.45, rgb(0xFFFFFF, 0.85)), (1, rgb(color))]),
                           start: hub, end: p, options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
    ctx.restoreGState()
}

// 目标节点：彩色圆，白色细圈，顶部一点高光
for (p, color) in targets {
    let r: CGFloat = 50
    let rect = CGRect(x: p.x - r, y: p.y - r, width: 2 * r, height: 2 * r)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: 10), blur: 26, color: rgb(0x000000, 0.35))
    ctx.setFillColor(rgb(color))
    ctx.fillEllipse(in: rect)
    ctx.restoreGState()
    ctx.saveGState()
    ctx.addEllipse(in: rect)
    ctx.clip()
    ctx.drawLinearGradient(gradient([(0, rgb(0xFFFFFF, 0.45)), (0.5, rgb(0xFFFFFF, 0))]),
                           start: CGPoint(x: p.x, y: rect.minY), end: CGPoint(x: p.x, y: rect.maxY), options: [])
    ctx.restoreGState()
    ctx.setStrokeColor(rgb(0xFFFFFF, 0.92))
    ctx.setLineWidth(9)
    ctx.strokeEllipse(in: rect.insetBy(dx: 4.5, dy: 4.5))
}

// 出口节点：白色大圆，深色的环和中心
do {
    let r: CGFloat = 92
    let rect = CGRect(x: hub.x - r, y: hub.y - r, width: 2 * r, height: 2 * r)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: 14), blur: 36, color: rgb(0x000000, 0.4))
    ctx.setFillColor(rgb(0xFFFFFF))
    ctx.fillEllipse(in: rect)
    ctx.restoreGState()
    ctx.saveGState()
    ctx.addEllipse(in: rect)
    ctx.clip()
    ctx.drawLinearGradient(gradient([(0, rgb(0xFFFFFF)), (1, rgb(0xDCE3FF))]),
                           start: CGPoint(x: hub.x, y: rect.minY), end: CGPoint(x: hub.x, y: rect.maxY), options: [])
    ctx.restoreGState()
    let ring = rect.insetBy(dx: 30, dy: 30)
    ctx.setStrokeColor(rgb(0x1A2266))
    ctx.setLineWidth(16)
    ctx.strokeEllipse(in: ring)
    let core = rect.insetBy(dx: 62, dy: 62)
    ctx.setFillColor(rgb(0x1A2266))
    ctx.fillEllipse(in: core)
}

// 玻璃质感：上半部一层很淡的白色高光
ctx.drawLinearGradient(gradient([(0, rgb(0xFFFFFF, 0.16)), (0.42, rgb(0xFFFFFF, 0.03)), (0.5, rgb(0xFFFFFF, 0))]),
                       start: CGPoint(x: 512, y: body.minY), end: CGPoint(x: 512, y: body.maxY), options: [])

// 内描边：让边缘在浅色背景上也清楚（画在圆角方形里面）
ctx.addPath(squircle)
ctx.setStrokeColor(rgb(0xFFFFFF, 0.14))
ctx.setLineWidth(6)
ctx.strokePath()
ctx.restoreGState()

guard let image = ctx.makeImage() else { fatalError("no image") }
let out = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon_1024.png")
guard let dest = CGImageDestinationCreateWithURL(out as CFURL, UTType.png.identifier as CFString, 1, nil) else {
    fatalError("cannot write \(out.path)")
}
CGImageDestinationAddImage(dest, image, nil)
guard CGImageDestinationFinalize(dest) else { fatalError("cannot write \(out.path)") }
