import SwiftUI

// 办公室里一个工位的插画（Canvas 画的，没有图片资源）：桌子、显示器、椅子、坐着的人（身子是账号色，脸是账号的头像）。
// 设计空间 240×168，按宽度缩放。显示器上演这个账号此刻在干什么；会动的部分（打字、滚动的代码、热气、zZ、汗）只在
// animate 时走（窗口看得见、没开“减少动态效果”），帧率看状态，没人、坐着陌生人的工位不走。
// 家具一律中性灰（不带含义），账号色只用在人、地毯和屏幕上。套餐的档位不画进来：看卡片上的徽章。

/// 人的姿势
enum DeskPose {
    /// 没人：椅子推开，账号色的外套搭在椅背上
    case away
    /// 坐着一个陌生人（登录的不是这个账号）
    case stranger
    /// 趴在桌上睡觉（额度用完了）
    case sleeping
    /// 扶着额头冒汗（线路、出口有问题）
    case sweating
    /// 前倾打字（在用）
    case typing
    /// 往后靠、端着杯子（空闲）
    case relaxed
}

/// 显示器上演的
enum DeskScreen: Equatable {
    case login(warn: Bool)
    case asleep
    case noSignal(severe: Bool)
    case standby
    /// 只有命令行
    case terminal
    /// 只有桌面 App
    case chat
    /// 两样都开着
    case split
}

extension Employee {
    var pose: DeskPose {
        switch mood {
        case .absent: return .away
        case .wrongPerson: return .stranger
        case .unwell: return .sweating
        case .resting: return .sleeping
        case .working: return .typing
        case .idle: return .relaxed
        }
    }

    var screen: DeskScreen {
        switch mood {
        case .absent: return .login(warn: false)
        case .wrongPerson: return .login(warn: true)
        case .unwell(let a): return .noSignal(severe: a.severe)
        case .resting: return .asleep
        case .working: return app && sessions > 0 ? .split : sessions > 0 ? .terminal : .chat
        case .idle: return .standby
        }
    }

    /// 动画的帧率；nil = 这个工位没什么会动的
    var deskFPS: Double? {
        switch pose {
        case .typing: return 10
        case .sleeping, .sweating: return 6
        case .relaxed: return 4
        case .away, .stranger: return nil
        }
    }
}

/// 一个工位。
struct DeskScene: View {
    let employee: Employee
    /// 让它动起来（窗口看得见、没开“减少动态效果”）
    let animate: Bool
    /// “几点回来”按这个时间算
    let now: Double

    @Environment(\.colorScheme) private var scheme

    static let design = CGSize(width: 240, height: 168)
    /// 不动的时候画这一刻（静止的那一帧也是设计过的）
    static let stillPhase = 0.37

    var body: some View {
        let e = employee
        let painter = DeskPainter(pose: e.pose, screen: e.screen, accent: e.account.lineColor,
                                  palette: OfficePalette(scheme),
                                  laptop: e.department == .contractors)
        GeometryReader { g in
            let s = g.size.width / Self.design.width
            let fps = e.deskFPS
            let running = animate && fps != nil
            ZStack(alignment: .topLeading) {
                TimelineView(.animation(minimumInterval: 1 / (fps ?? 1), paused: !running)) { ctx in
                    let t = running ? ctx.date.timeIntervalSinceReferenceDate : Self.stillPhase
                    Canvas { c, _ in
                        c.scaleBy(x: s, y: s)
                        painter.draw(&c, t: t)
                    }
                }
                faces(painter, s)
                screenText(painter, s)
            }
        }
        .aspectRatio(Self.design.width / Self.design.height, contentMode: .fit)
        .accessibilityHidden(true)
    }

    /// 脸（账号的头像）；陌生人是灰色的问号，真主人的头像别在显示器角上；没人时别在外套上。
    @ViewBuilder
    private func faces(_ p: DeskPainter, _ s: CGFloat) -> some View {
        let a = employee.account
        switch p.pose {
        case .away:
            BadgeAvatar(account: a, size: 15 * s)
                .position(x: (DeskPainter.coat.midX + p.chairShift) * s, y: (DeskPainter.coat.minY + 18) * s)
        case .stranger:
            ZStack {
                Circle().fill(p.palette.stranger)
                Text("?").font(.system(size: 15 * s, weight: .bold, design: .rounded)).foregroundStyle(.white)
            }
            .frame(width: 30 * s, height: 30 * s)
            .position(x: p.head.x * s, y: p.head.y * s)
            BadgeAvatar(account: a, size: 16 * s)
                .position(x: (p.monitor.maxX - 3) * s, y: (p.monitor.minY + 3) * s)
        default:
            BadgeAvatar(account: a, size: p.headSize * s)
                .rotationEffect(.degrees(p.pose == .sleeping ? -14 : 0))
                .position(x: p.head.x * s, y: p.head.y * s)
        }
    }

    /// 睡着时显示器上写几点回来
    @ViewBuilder
    private func screenText(_ p: DeskPainter, _ s: CGFloat) -> some View {
        if case .resting(let until?) = employee.mood {
            Text(Texts.backTime(epoch: until, now: now))
                .font(.system(size: 8.5 * s, weight: .semibold, design: .rounded))
                .monospacedDigit()
                .lineLimit(1)
                .minimumScaleFactor(0.6)
                .foregroundStyle(OfficePalette.moon)
                .frame(width: (p.screenRect.width - 8) * s)
                .position(x: p.screenRect.midX * s, y: (p.screenRect.midY + 9) * s)
        }
    }
}

/// 工位的配色。家具在浅色、深色下各一套中性灰；屏幕本来就是深色的，两种外观一样。
struct OfficePalette {
    let deskTop, deskFront, drawer, chairBack, chairSeat, chairPole, frame, keyboard, mug, steam, zzz, pants, stranger: Color
    let dark: Bool

    static let moon = Color(hex: 0xB3C2DD)
    static let screenOff = Color(hex: 0x07090C)
    static let screenLogin = Color(hex: 0x1B2A44)
    static let screenStandby = Color(hex: 0x10151C)
    static let screenTerminal = Color(hex: 0x0D1319)
    static let screenChat = Color(hex: 0x161E29)
    static let screenNoSignal = Color(hex: 0x1C1517)
    static let codeGrey = Color(hex: 0x9AA7B8)
    static let sweat = Color(hex: 0x9FB4C7)

    init(_ scheme: ColorScheme) {
        dark = scheme == .dark
        if dark {
            deskTop = Color(hex: 0x77756F); deskFront = Color(hex: 0x615F5A); drawer = Color(hex: 0x4E4C48)
            chairBack = Color(hex: 0x8D96A3); chairSeat = Color(hex: 0x707A88); chairPole = Color(hex: 0xA3ADBB)
            frame = Color(hex: 0x2A2F37); keyboard = Color(hex: 0x9AA1AD); mug = Color(hex: 0xD9DBE0)
            steam = Color(hex: 0xE5E9F0).opacity(0.5); zzz = Color(hex: 0xB3C2DD); pants = Color(hex: 0x4B5260)
            stranger = Color(hex: 0x6E7580)
        } else {
            deskTop = Color(hex: 0xD9D6D0); deskFront = Color(hex: 0xC4C0B8); drawer = Color(hex: 0xADA8A0)
            chairBack = Color(hex: 0x4B525C); chairSeat = Color(hex: 0x3A4048); chairPole = Color(hex: 0x6B7280)
            frame = Color(hex: 0x252A32); keyboard = Color(hex: 0x6B7280); mug = Color(hex: 0xEDEEF0)
            steam = Color(hex: 0x6B7280).opacity(0.55); zzz = Color(hex: 0x5C6B86); pants = Color(hex: 0x3A4048)
            stranger = Color(hex: 0x8B929C)
        }
    }
}

/// 画一个工位（纯计算，t 是秒）。
struct DeskPainter {
    let pose: DeskPose
    let screen: DeskScreen
    let accent: Color
    let palette: OfficePalette
    /// 外包（第三方 API 账号）：桌上是笔记本，不是显示器
    let laptop: Bool

    // MARK: 几何（设计空间 240×168，y 向下）

    static let coat = CGRect(x: 186, y: 50, width: 32, height: 50)
    private static let codeWidths: [CGFloat] = [0.62, 0.40, 0.78, 0.30, 0.55, 0.70, 0.45, 0.36, 0.66, 0.50, 0.28, 0.58]

    /// 没人时椅子往后推一点
    var chairShift: CGFloat { pose == .away ? 14 : 0 }

    var monitor: CGRect { laptop ? CGRect(x: 52, y: 58, width: 62, height: 44) : CGRect(x: 30, y: 34, width: 90, height: 62) }
    var screenRect: CGRect { monitor.insetBy(dx: laptop ? 3 : 4, dy: laptop ? 3 : 4) }

    var hip: CGPoint { CGPoint(x: pose == .relaxed ? 193 : 190, y: 104) }

    var shoulder: CGPoint {
        switch pose {
        case .typing, .stranger: return CGPoint(x: 179, y: 68)
        case .relaxed: return CGPoint(x: 197, y: 67)
        case .sleeping: return CGPoint(x: 165, y: 88)
        case .sweating: return CGPoint(x: 186, y: 67)
        case .away: return .zero
        }
    }

    var head: CGPoint {
        switch pose {
        case .typing, .stranger: return CGPoint(x: 171, y: 46)
        case .relaxed: return CGPoint(x: 194, y: 44)
        case .sleeping: return CGPoint(x: 146, y: 94)
        case .sweating: return CGPoint(x: 184, y: 45)
        case .away: return .zero
        }
    }

    var headSize: CGFloat { pose == .sleeping ? 26 : 30 }

    /// 手臂：从肩到手（elbow 是二次曲线的控制点）
    private func arm(_ t: Double) -> (from: CGPoint, elbow: CGPoint, hand: CGPoint)? {
        switch pose {
        case .typing:
            let bob = CGFloat(1.4 * sin(2 * .pi * 3 * t))
            return (CGPoint(x: 177, y: 74), CGPoint(x: 158, y: 102), CGPoint(x: 128, y: 102 + bob))
        case .stranger:
            return (CGPoint(x: 177, y: 74), CGPoint(x: 158, y: 102), CGPoint(x: 128, y: 102))
        case .relaxed:
            return (CGPoint(x: 195, y: 74), CGPoint(x: 182, y: 104), CGPoint(x: 160, y: 100))
        case .sleeping:
            return (CGPoint(x: 166, y: 92), CGPoint(x: 150, y: 110), CGPoint(x: 126, y: 105))
        case .sweating:
            return (CGPoint(x: 182, y: 74), CGPoint(x: 160, y: 74), CGPoint(x: 170, y: 51))
        case .away:
            return nil
        }
    }

    private func rr(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat, _ r: CGFloat) -> Path {
        Path(roundedRect: CGRect(x: x, y: y, width: w, height: h), cornerRadius: r, style: .continuous)
    }

    private var armColor: Color { accent.mix(with: .black, by: 0.2) }

    // MARK: 画

    func draw(_ c: inout GraphicsContext, t: Double) {
        // 地毯
        c.fill(Path(ellipseIn: CGRect(x: 54, y: 143, width: 180, height: 15)), with: .color(accent.opacity(palette.dark ? 0.26 : 0.15)))
        drawChairBack(&c)
        if pose != .away { drawBody(&c) }
        drawChairSeat(&c)
        if pose != .away { drawLegs(&c) }
        if pose == .away { drawCoat(&c) }
        drawDesk(&c)
        drawMonitor(&c, t: t)
        drawKeyboardAndMug(&c)
        if let a = arm(t) {
            var p = Path()
            p.move(to: a.from)
            p.addQuadCurve(to: a.hand, control: a.elbow)
            c.stroke(p, with: .color(pose == .stranger ? palette.stranger.mix(with: .black, by: 0.2) : armColor),
                     style: StrokeStyle(lineWidth: 7, lineCap: .round))
        }
        switch pose {
        case .typing, .relaxed: drawSteam(&c, t: t)
        case .sleeping: drawZzz(&c, t: t)
        case .sweating: drawSweat(&c, t: t)
        default: break
        }
    }

    private func drawChairBack(_ c: inout GraphicsContext) {
        c.fill(rr(182 + chairShift, 46, 32, 58, 12), with: .color(palette.chairBack))
    }

    private func drawChairSeat(_ c: inout GraphicsContext) {
        let dx = chairShift
        c.fill(rr(166 + dx, 102, 50, 8, 4), with: .color(palette.chairSeat))
        c.fill(rr(189 + dx, 110, 4, 27, 1), with: .color(palette.chairPole))
        var base = Path()
        base.move(to: CGPoint(x: 172 + dx, y: 139))
        base.addLine(to: CGPoint(x: 210 + dx, y: 139))
        c.stroke(base, with: .color(palette.chairPole), style: StrokeStyle(lineWidth: 3.5, lineCap: .round))
        for x in [174.0, 191.0, 208.0] {
            c.fill(Path(ellipseIn: CGRect(x: x + dx - 3, y: 140, width: 6, height: 6)), with: .color(palette.chairSeat))
        }
    }

    private func drawBody(_ c: inout GraphicsContext) {
        var torso = Path()
        torso.move(to: hip)
        torso.addLine(to: shoulder)
        let color = pose == .stranger ? palette.stranger : accent
        c.stroke(torso, with: .color(color), style: StrokeStyle(lineWidth: 26, lineCap: .round))
    }

    private func drawLegs(_ c: inout GraphicsContext) {
        var leg = Path()
        leg.move(to: CGPoint(x: hip.x - 2, y: 106))
        leg.addLine(to: CGPoint(x: 164, y: 110))
        leg.addLine(to: CGPoint(x: 163, y: 135))
        c.stroke(leg, with: .color(palette.pants), style: StrokeStyle(lineWidth: 9, lineCap: .round, lineJoin: .round))
    }

    private func drawCoat(_ c: inout GraphicsContext) {
        let r = Self.coat.offsetBy(dx: chairShift, dy: 0)
        c.fill(rr(r.minX, r.minY, r.width, r.height, 10), with: .color(accent))
        var lapel = Path()
        lapel.move(to: CGPoint(x: r.minX + 8, y: r.minY + 2))
        lapel.addLine(to: CGPoint(x: r.midX, y: r.minY + 16))
        lapel.addLine(to: CGPoint(x: r.maxX - 8, y: r.minY + 2))
        c.stroke(lapel, with: .color(accent.mix(with: .black, by: 0.3)), style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
    }

    private func drawDesk(_ c: inout GraphicsContext) {
        c.fill(rr(20, 116, 142, 34, 3), with: .color(palette.deskFront))
        c.fill(rr(12, 108, 158, 8, 3), with: .color(palette.deskTop))
        // 抽屉
        c.stroke(rr(116, 121, 40, 12, 2), with: .color(palette.drawer), lineWidth: 1)
        c.stroke(rr(116, 136, 40, 11, 2), with: .color(palette.drawer), lineWidth: 1)
        for y in [127.0, 141.5] {
            c.fill(rr(131, y - 1, 10, 2, 1), with: .color(palette.drawer))
        }
    }

    private func drawMonitor(_ c: inout GraphicsContext, t: Double) {
        let m = monitor
        if laptop {
            // 笔记本：屏幕 + 薄薄的底座
            c.fill(rr(m.minX, m.minY, m.width, m.height, 4), with: .color(palette.frame))
            c.fill(rr(m.minX - 7, 102, m.width + 14, 6, 2), with: .color(palette.keyboard.opacity(0.9)))
        } else {
            c.fill(rr(m.minX, m.minY, m.width, m.height, 7), with: .color(palette.frame))
            if palette.dark { c.stroke(rr(m.minX, m.minY, m.width, m.height, 7), with: .color(.white.opacity(0.3)), lineWidth: 0.6) }
            c.fill(rr(m.midX - 4, m.maxY, 8, 8, 1), with: .color(palette.frame))
            c.fill(rr(m.midX - 19, m.maxY + 8, 38, 4, 2), with: .color(palette.frame))
        }
        let r = screenRect
        let shape = rr(r.minX, r.minY, r.width, r.height, laptop ? 2 : 4)
        var sc = c
        sc.clip(to: shape)
        switch screen {
        case .login(let warn):
            sc.fill(shape, with: .color(OfficePalette.screenLogin))
            drawLock(&sc, center: CGPoint(x: r.midX, y: r.midY), color: warn ? .orange : .white.opacity(0.9))
        case .asleep:
            sc.fill(shape, with: .color(OfficePalette.screenOff))
            var moon = Path(ellipseIn: CGRect(x: r.midX - 6, y: r.minY + 7, width: 12, height: 12))
            moon.addEllipse(in: CGRect(x: r.midX - 2, y: r.minY + 4.5, width: 12, height: 12))
            sc.fill(moon, with: .color(OfficePalette.moon), style: FillStyle(eoFill: true))
        case .noSignal(let severe):
            sc.fill(shape, with: .color(OfficePalette.screenNoSignal))
            drawWarning(&sc, center: CGPoint(x: r.midX, y: r.midY), color: severe ? .red : .orange)
        case .standby:
            sc.fill(shape, with: .color(OfficePalette.screenStandby))
            drawPower(&sc, center: CGPoint(x: r.midX, y: r.midY))
        case .terminal:
            sc.fill(shape, with: .color(OfficePalette.screenTerminal))
            drawCode(&sc, in: r, t: t)
        case .chat:
            sc.fill(shape, with: .color(OfficePalette.screenChat))
            drawChat(&sc, in: r, t: t)
        case .split:
            let left = CGRect(x: r.minX, y: r.minY, width: r.width / 2, height: r.height)
            let right = CGRect(x: r.midX, y: r.minY, width: r.width / 2, height: r.height)
            sc.fill(Path(left), with: .color(OfficePalette.screenTerminal))
            sc.fill(Path(right), with: .color(OfficePalette.screenChat))
            var l = sc
            l.clip(to: Path(left))
            drawCode(&l, in: left, t: t)
            var rc = sc
            rc.clip(to: Path(right))
            drawChat(&rc, in: right, t: t)
            var div = Path()
            div.move(to: CGPoint(x: r.midX, y: r.minY))
            div.addLine(to: CGPoint(x: r.midX, y: r.maxY))
            sc.stroke(div, with: .color(.white.opacity(0.12)), lineWidth: 0.8)
        }
    }

    private func drawKeyboardAndMug(_ c: inout GraphicsContext) {
        if !laptop { c.fill(rr(98, 103, 44, 5, 2), with: .color(palette.keyboard)) }
        c.fill(rr(148, 96, 12, 12, 2.5), with: .color(palette.mug))
        var handle = Path()
        handle.addArc(center: CGPoint(x: 160, y: 102), radius: 3.5, startAngle: .degrees(-80), endAngle: .degrees(80), clockwise: false)
        c.stroke(handle, with: .color(palette.mug), lineWidth: 1.8)
    }

    // MARK: 屏幕里

    /// 滚动的代码：账号色和灰色的行
    private func drawCode(_ c: inout GraphicsContext, in r: CGRect, t: Double) {
        let lineH: CGFloat = 7
        let travel = CGFloat(t * 12)
        let start = Int(travel / lineH)
        let offset = travel.truncatingRemainder(dividingBy: lineH)
        let count = Int(r.height / lineH) + 2
        for i in 0..<count {
            let idx = start + i
            let y = r.minY + 5 + CGFloat(i) * lineH - offset
            let indent: CGFloat = idx % 4 == 2 ? 6 : 0
            let w = (r.width - 12 - indent) * Self.codeWidths[idx % Self.codeWidths.count]
            let color = idx % 3 == 0 ? accent.opacity(0.95) : OfficePalette.codeGrey.opacity(0.8)
            c.fill(rr(r.minX + 5 + indent, y, w, 3, 1.5), with: .color(color))
        }
        if Int(t * 2) % 2 == 0 {
            c.fill(rr(r.minX + 5, r.maxY - 8, 4, 4, 0.5), with: .color(accent))
        }
    }

    /// 聊天：对方的灰气泡、自己的账号色气泡、对方正在输入的三个点
    private func drawChat(_ c: inout GraphicsContext, in r: CGRect, t: Double) {
        let h: CGFloat = 9
        c.fill(rr(r.minX + 5, r.minY + 6, r.width * 0.5, h, 4.5), with: .color(OfficePalette.codeGrey.opacity(0.55)))
        c.fill(rr(r.maxX - 5 - r.width * 0.56, r.minY + 19, r.width * 0.56, h, 4.5), with: .color(accent.opacity(0.9)))
        let typing = CGRect(x: r.minX + 5, y: r.minY + 32, width: 22, height: h)
        if typing.maxY < r.maxY {
            c.fill(rr(typing.minX, typing.minY, typing.width, typing.height, 4.5), with: .color(OfficePalette.codeGrey.opacity(0.55)))
            for k in 0..<3 {
                let a = 0.3 + 0.7 * max(0, sin(2 * .pi * (1.5 * t - Double(k) / 3)))
                c.fill(Path(ellipseIn: CGRect(x: typing.minX + 5 + CGFloat(k) * 5, y: typing.midY - 1.25, width: 2.5, height: 2.5)),
                       with: .color(.white.opacity(a)))
            }
        }
    }

    private func drawLock(_ c: inout GraphicsContext, center p: CGPoint, color: Color) {
        c.fill(rr(p.x - 7, p.y - 1, 14, 10, 2), with: .color(color))
        var shackle = Path()
        shackle.addArc(center: CGPoint(x: p.x, y: p.y - 1), radius: 4.5, startAngle: .degrees(180), endAngle: .degrees(0), clockwise: false)
        c.stroke(shackle, with: .color(color), lineWidth: 2)
    }

    private func drawWarning(_ c: inout GraphicsContext, center p: CGPoint, color: Color) {
        var tri = Path()
        tri.move(to: CGPoint(x: p.x, y: p.y - 9))
        tri.addLine(to: CGPoint(x: p.x + 9.5, y: p.y + 7))
        tri.addLine(to: CGPoint(x: p.x - 9.5, y: p.y + 7))
        tri.closeSubpath()
        c.stroke(tri, with: .color(color), style: StrokeStyle(lineWidth: 2, lineJoin: .round))
        c.fill(rr(p.x - 1, p.y - 4, 2, 6, 1), with: .color(color))
        c.fill(Path(ellipseIn: CGRect(x: p.x - 1.2, y: p.y + 3, width: 2.4, height: 2.4)), with: .color(color))
    }

    private func drawPower(_ c: inout GraphicsContext, center p: CGPoint) {
        var ring = Path()
        ring.addArc(center: p, radius: 6.5, startAngle: .degrees(-60), endAngle: .degrees(240), clockwise: false)
        c.stroke(ring, with: .color(accent.opacity(0.85)), style: StrokeStyle(lineWidth: 1.8, lineCap: .round))
        var line = Path()
        line.move(to: CGPoint(x: p.x, y: p.y - 8))
        line.addLine(to: CGPoint(x: p.x, y: p.y - 2))
        c.stroke(line, with: .color(accent.opacity(0.85)), style: StrokeStyle(lineWidth: 1.8, lineCap: .round))
    }

    // MARK: 效果

    private func drawSteam(_ c: inout GraphicsContext, t: Double) {
        for k in 0..<3 {
            let p = (t / 2.4 + Double(k) / 3).truncatingRemainder(dividingBy: 1)
            let x = 154 + 2.5 * sin(2 * .pi * p + Double(k))
            let y = 93 - 14 * p
            let r = 1.6 + 1.2 * p
            c.fill(Path(ellipseIn: CGRect(x: x - r, y: y - r, width: r * 2, height: r * 2)), with: .color(palette.steam.opacity(1 - p)))
        }
    }

    private func drawZzz(_ c: inout GraphicsContext, t: Double) {
        for k in 0..<3 {
            let p = (t / 3.2 + Double(k) / 3).truncatingRemainder(dividingBy: 1)
            let size = 5 * (0.6 + 0.5 * p)
            let o = CGPoint(x: 156 + 14 * p, y: 78 - 22 * p)
            var z = Path()
            z.move(to: o)
            z.addLine(to: CGPoint(x: o.x + size, y: o.y))
            z.addLine(to: CGPoint(x: o.x, y: o.y + size))
            z.addLine(to: CGPoint(x: o.x + size, y: o.y + size))
            c.stroke(z, with: .color(palette.zzz.opacity(sin(.pi * p))), style: StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
        }
    }

    private func drawSweat(_ c: inout GraphicsContext, t: Double) {
        let p = (t / 1.4).truncatingRemainder(dividingBy: 1)
        let o = CGPoint(x: 202, y: 36 + 10 * p)
        var drop = Path()
        drop.move(to: CGPoint(x: o.x, y: o.y - 4))
        drop.addQuadCurve(to: CGPoint(x: o.x, y: o.y + 3), control: CGPoint(x: o.x + 5, y: o.y + 3))
        drop.addQuadCurve(to: CGPoint(x: o.x, y: o.y - 4), control: CGPoint(x: o.x - 5, y: o.y + 3))
        c.fill(drop, with: .color(OfficePalette.sweat.opacity(1 - p)))
    }
}
