import SwiftUI

// 给新手的引导：第一次打开时的卡片教程（一步一张，可以随时从“帮助”菜单、工具栏的问号再看），升级后第一次打开时的
// “更新了什么”（一个版本一张，内容在 CcaKit 的 WhatsNew.swift，“帮助”菜单的「更新说明」能再看），
// 页面上的提示卡片（讲一句这一页怎么看，点“知道了”就不再出现），账号页顶上“现在能不能用、下一步点什么”的一句话，
// 以及默认收起的技术细节（端口、槽位、纯净度明细这些，点“显示细节”才出来）。

/// 引导的状态：教程看过没有、看过哪个版本的更新说明、关掉了哪些提示卡片、要不要显示技术细节。
/// 记在 App 的设置里（演示模式只记在内存里）。
@MainActor
@Observable
final class Guide {
    private enum Keys {
        static let done = "guide.tutorialDone"
        static let tips = "guide.dismissedTips"
        static let details = "guide.showDetails"
        /// 看过哪个版本的“更新了什么”（App 的版本号）
        static let whatsNewSeen = "guide.whatsNewSeen"
    }

    @ObservationIgnored private let prefs: PrefsStore
    /// 这个 App 的版本号（更新说明按它挑）
    @ObservationIgnored let appVersion: String
    /// 主窗口上正在看的引导卡片：新手教程，或者这几个版本的更新说明（最新的在前）
    var sheet: Sheet?
    /// 现在在第几张（从 0 起）
    var step = 0
    private(set) var dismissedTips: Set<String>
    /// 显示技术细节（端口、槽位、出口检测的明细）
    private(set) var showDetails: Bool

    enum Sheet: Identifiable, Equatable {
        case tutorial
        case whatsNew([ReleaseNote])

        var id: String {
            if case .tutorial = self { return "tutorial" }
            return "whatsNew"
        }
    }

    init(prefs: PrefsStore, appVersion: String) {
        self.prefs = prefs
        self.appVersion = appVersion
        dismissedTips = Set((prefs.string(forKey: Keys.tips) ?? "").split(separator: ",").map(String.init))
        showDetails = prefs.bool(forKey: Keys.details)
    }

    /// 看过（或者跳过过）卡片教程了吗。
    var tutorialDone: Bool { prefs.bool(forKey: Keys.done) }

    /// 第一次连上 cca 时：第一次装就打开教程（看过、跳过过的不再自动打开）；升级了就给这几个版本更新了什么
    /// （WhatsNew.onLaunch：给了就记作看过，以后不再自动给）。
    func startIfNeeded() {
        guard sheet == nil else { return }
        let done = prefs.bool(forKey: Keys.done)
        let r = WhatsNew.onLaunch(tutorialDone: done, lastSeen: prefs.string(forKey: Keys.whatsNewSeen), current: appVersion)
        if let seen = r.seen { prefs.set(seen, forKey: Keys.whatsNewSeen) }
        if !done {
            openTutorial()
        } else if !r.show.isEmpty {
            showWhatsNew(r.show)
        }
    }

    func openTutorial() {
        step = 0
        sheet = .tutorial
    }

    /// 看完或跳过：记下来，以后不再自动打开。
    func finishTutorial() {
        prefs.set(true, forKey: Keys.done)
        if sheet == .tutorial { sheet = nil }
    }

    /// 「帮助」菜单的「更新说明」：不比这个版本新的都能翻（最新的在前）。
    func openWhatsNew() {
        showWhatsNew(WhatsNew.history(current: appVersion))
    }

    func showWhatsNew(_ notes: [ReleaseNote]) {
        guard !notes.isEmpty else { return }
        step = 0
        sheet = .whatsNew(notes)
    }

    /// 正在看的更新说明（没在看是空的）
    var whatsNewNotes: [ReleaseNote] {
        if case .whatsNew(let notes)? = sheet { return notes }
        return []
    }

    /// 弹层关掉了（按钮、Esc）：教程记作看过；更新说明收起
    func dismissSheet() {
        if sheet == .tutorial { finishTutorial() } else { sheet = nil }
    }

    func isDismissed(_ tip: String) -> Bool { dismissedTips.contains(tip) }

    func dismiss(_ tip: String) {
        dismissedTips.insert(tip)
        prefs.set(dismissedTips.sorted().joined(separator: ","), forKey: Keys.tips)
    }

    /// 帮助菜单的“重新显示提示卡片”。
    func resetTips() {
        dismissedTips = []
        prefs.set("", forKey: Keys.tips)
    }

    func toggleDetails() {
        showDetails.toggle()
        prefs.set(showDetails, forKey: Keys.details)
    }
}

// MARK: 卡片教程

/// 教程的一张卡片：上面一幅小图（用 App 里真实的部件画），下面标题和几条带图标的要点。
struct TutorialCard {
    let title: String
    let points: [(String, String)]
    let art: Art

    /// 小图；更新说明（ReleaseNote.art）按名字找，认不出就画 Logo
    enum Art: String { case fan, office, sidebar, open, quota, route, map, rename, menubar, badges, logo }

    static let all: [TutorialCard] = [
        TutorialCard(title: "一台 Mac，好几个账号", points: [
            ("person.2", "同时管好几个 Claude、ChatGPT 账号：总览里一眼看全，左边按工具分开列出。"),
            ("point.3.connected.trianglepath.dotted", "每个账号有自己的登录资料和网络出口，启动前 cca 会先检查。"),
            ("lock.shield", "每个账号用自己的登录和浏览器资料，换账号不用重新登录。"),
        ], art: .fan),
        TutorialCard(title: "总览：哪个在用，哪个额度用完了", points: [
            ("square.grid.2x2", "打开 App 先看到总览：每个账号一张卡片，Claude、ChatGPT、第三方 API 账号分开排好。"),
            ("desktopcomputer", "图里的屏幕在滚代码、聊天，说明这个账号正在用（App 开着，或者终端里有会话）。"),
            ("moon.zzz", "趴在桌上是额度用完了，屏幕上写着几点恢复；椅子空着是没登录，坐着灰色的人是登录的不是这个账号。"),
            ("sparkle", "套餐看徽章前面的小记号：灰圈、蓝点、紫色菱形，最高档（Ultra、Pro 500）是彩色的四角星。点一张卡片就进这个账号的页面。"),
            ("chart.bar.xaxis", "左边的「用量统计」把每个账号每周用了多少排在一起：哪个总不够用、哪个常常剩一大半，一眼看出来；切到「折合金额」看按官方 API 的价格折成多少美元。"),
        ], art: .office),
        TutorialCard(title: "左边是你的账号", points: [
            ("cursorarrow.click", "点一个账号，右边就是它的详情；顶上的「Claude / ChatGPT」切换工具（用了 Antigravity 命令行时还有它）。"),
            ("chart.bar", "名字右边的小条是额度：5 小时、本周，Claude 账号还有一根 Fable；没有 5 小时限制的套餐（比如 Ultra）没有 5h 那一根。"),
            ("circle.fill", "头像右下角有绿点，说明这个账号正在用（App 开着，或者终端里有会话）；最高档套餐的头像外面有一圈细细的光晕。"),
            ("contextualmenu.and.cursorarrow", "在账号上点右键：打开 App、网页版、终端、检测出口、改别名、置顶、重新登录。上下拖动可以调整顺序。"),
        ], art: .sidebar),
        TutorialCard(title: "打开一个账号", points: [
            ("macwindow", "点「打开 Claude App」：用这个账号自己的出口和资料打开。"),
            ("safari", "点「网页版」：在账号专属 Chrome 资料里打开；未安装 Chrome 时会停止打开。"),
            ("terminal", "点「终端」：在选好的文件夹里用这个账号开一个会话，下次还用这个文件夹；右边的箭头可以换文件夹、换终端。"),
            ("checkmark.shield", "打开前 cca 会先检查出口；有问题时直接告诉你该点哪个按钮。"),
        ], art: .open),
        TutorialCard(title: "看额度还剩多少", points: [
            ("gauge.with.dots.needle.33percent", "额度分「5 小时」和「本周」两块，没有 5 小时限制的套餐（比如 Ultra）只有本周。Claude 的 Fable 单独一块：它只能用掉本周额度的一部分，用完了别的模型还能接着用。"),
            ("exclamationmark.triangle", "快用完时变橙、变红；到了重置时间自动恢复。"),
            ("hourglass", "本周额度快重置了、还剩不少的账号，名字旁边有个沙漏：建议先用它，别浪费。"),
            ("arrow.counterclockwise", "ChatGPT 账号还有重置次数时，点「重置额度…」能把用量清零（会先问你）。"),
            ("arrow.clockwise", "点「刷新用量」，经这个账号的出口查一次最新的。"),
        ], art: .quota),
        TutorialCard(title: "看网络出口", points: [
            ("arrow.triangle.branch", "这条线就是这个账号的流量：这台 Mac → 节点 → 出口 IP。"),
            ("checkmark.shield", "第一次用某个账号，先点「检测出口」：看看出口在哪、纯净度怎么样、DNS / WebRTC 有没有泄露风险。之后会在后台定时重测。"),
            ("slider.horizontal.3", "想看端口、纯净度这些明细，点「显示细节」。"),
        ], art: .route),
        TutorialCard(title: "节点分流", points: [
            ("wifi.router", "「节点分流」页画出每个账号经路由器连到哪个节点。"),
            ("line.diagonal", "实线是正常的；虚线说明节点选错了，或者端口连不上。"),
            ("arrow.triangle.2.circlepath", "有问题时点「同步路由器」就能修好（要重载路由器时会先问你）。"),
        ], art: .map),
        TutorialCard(title: "改别名、换外观", points: [
            ("pencil", "直接点账号页上的名字就能改别名：回车保存，Esc 取消。"),
            ("circle.lefthalf.filled", "左下角的按钮切换浅色、深色，或者跟随系统。"),
            ("plus", "添加账号：点左边账号列表上的「＋」；配置路由器、本机 Clash：点工具栏的「＋」，按提示完成。"),
        ], art: .rename),
        TutorialCard(title: "菜单栏、更新和帮助", points: [
            ("menubar.rectangle", "屏幕顶上菜单栏的 cca 图标：不开主窗口也能看额度、一键打开账号。关掉主窗口后 cca 还在后台运行，要彻底退出点这里面的「退出」。"),
            ("arrow.down.circle", "cca 有新版本时窗口顶上会提示，点「更新并重新打开」；点左下角的版本号随时检查 cca 更新。"),
            ("arrow.down.app", "桌面 App 和终端里的 Claude Code、Codex 的更新在左侧「客户端更新」页：自动检查，默认你确认后才安装；也可以打开「允许自动更新」。"),
            ("book", "想再看一遍这个教程：点左下角的「新手教程」或工具栏的问号；「帮助」菜单里还有每个版本的更新说明。"),
        ], art: .menubar),
    ]
}

/// 卡片弹层（新手教程、更新说明共用）：上面一幅小图，下面一行小字、标题和几条带图标的要点，最下面是翻页的小圆点和按钮。
struct CardSheet<Footer: View>: View {
    /// 读屏软件念的名字（新手教程、更新说明）
    let name: String
    let art: TutorialCard.Art
    let caption: String
    let title: String
    let points: [(String, String)]
    let page: Int
    let pages: Int
    let select: (Int) -> Void
    @ViewBuilder let footer: Footer

    var body: some View {
        VStack(spacing: 0) {
            TutorialArt(art: art)
                .frame(maxWidth: .infinity)
                .frame(height: 196)
                .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Surface.tile))
                .padding([.horizontal, .top], 24)
                .id(page)
                .transition(.opacity)
            VStack(alignment: .leading, spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(caption)
                        .font(.caption.weight(.medium).monospacedDigit())
                        .foregroundStyle(.secondary)
                    Text(title).font(.title2.weight(.semibold))
                }
                VStack(alignment: .leading, spacing: 9) {
                    ForEach(points.indices, id: \.self) { k in
                        HStack(alignment: .firstTextBaseline, spacing: 10) {
                            Image(systemName: points[k].0)
                                .font(.callout.weight(.semibold))
                                // “App 开着”的绿点照真实的样子画成绿色，其它要点用强调色
                                .foregroundStyle(points[k].0 == "circle.fill" ? AnyShapeStyle(Color.green) : AnyShapeStyle(.tint))
                                .frame(width: 20)
                            Text(points[k].1)
                                .font(.body)
                                .foregroundStyle(.primary.opacity(0.85))
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 28)
            .padding(.top, 18)
            Spacer(minLength: 10)
            HStack(spacing: 10) {
                if pages > 1 {
                    PageDots(count: pages, current: page, select: select)
                }
                Spacer()
                footer
            }
            .controlSize(.large)
            .padding(.horizontal, 24)
            .padding(.bottom, 22)
        }
        .frame(width: 600, height: 512)
        .animation(.snappy(duration: 0.25), value: page)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(name)，\(caption)：\(title)")
    }
}

/// 卡片教程（弹层）：一步一张；第一张可以跳过，最后一张能“从头再看”或“开始使用”。
struct TutorialSheet: View {
    @Environment(Guide.self) private var guide

    var body: some View {
        let cards = TutorialCard.all
        let i = min(max(guide.step, 0), cards.count - 1)
        let card = cards[i]
        let last = i == cards.count - 1
        CardSheet(name: "新手教程", art: card.art, caption: "第 \(i + 1) 步，共 \(cards.count) 步", title: card.title,
                  points: card.points, page: i, pages: cards.count, select: { guide.step = $0 }) {
            if last {
                Button {
                    guide.step = 0
                } label: {
                    Label("从头再看", systemImage: "arrow.counterclockwise")
                }
            } else if i == 0 {
                Button("跳过") { guide.finishTutorial() }
                    .keyboardShortcut(.cancelAction)
            }
            if i > 0 {
                Button("上一步") { guide.step = i - 1 }
            }
            Button(last ? "开始使用" : "下一步") {
                if last { guide.finishTutorial() } else { guide.step = i + 1 }
            }
            .buttonStyle(.glassProminent)
            .keyboardShortcut(.defaultAction)
        }
    }
}

/// 更新了什么（弹层）：一个版本一张（最新的在前），样子和新手教程一样。
struct WhatsNewSheet: View {
    @Environment(Guide.self) private var guide

    var body: some View {
        let notes = guide.whatsNewNotes
        if !notes.isEmpty {
            let i = min(max(guide.step, 0), notes.count - 1)
            let note = notes[i]
            let last = i == notes.count - 1
            CardSheet(name: "更新说明", art: TutorialCard.Art(rawValue: note.art) ?? .logo, caption: "\(note.version) 更新了什么",
                      title: note.title, points: note.points.map { ($0.symbol, $0.text) }, page: i, pages: notes.count,
                      select: { guide.step = $0 }) {
                if !last {
                    Button("关闭") { guide.dismissSheet() }
                        .keyboardShortcut(.cancelAction)
                }
                if i > 0 {
                    Button("上一张") { guide.step = i - 1 }
                }
                Button(last ? "知道了" : "更早的版本") {
                    if last { guide.dismissSheet() } else { guide.step = i + 1 }
                }
                .buttonStyle(.glassProminent)
                .keyboardShortcut(.defaultAction)
            }
        }
    }
}

/// 第几步的点：现在这一步是一根短条；点哪个点就跳到哪一步。
struct PageDots: View {
    let count: Int
    let current: Int
    let select: (Int) -> Void

    var body: some View {
        HStack(spacing: 4) {
            ForEach(0..<count, id: \.self) { i in
                Button {
                    select(i)
                } label: {
                    Capsule()
                        .fill(i == current ? AnyShapeStyle(.primary) : AnyShapeStyle(.quaternary))
                        .frame(width: i == current ? 18 : 7, height: 7)
                        .padding(.vertical, 6)
                        .padding(.horizontal, 2)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("第 \(i + 1) 步")
                .accessibilityLabel("第 \(i + 1) 步")
            }
        }
    }
}

/// 教程卡片上的小图：都用 App 里真实的部件和账号色画（示意，不能点）。
struct TutorialArt: View {
    let art: TutorialCard.Art

    private static let sample: [(String, UInt32, String)] = [("工", 0x30A46C, "工作"), ("C", 0x8E4EC6, "carol"), ("F", 0x0A84FF, "frank")]

    var body: some View {
        Group {
            switch art {
            case .fan: fanArt
            case .office: officeArt
            case .sidebar: sidebarArt
            case .open: openArt
            case .quota: quotaArt
            case .route: routeArt
            case .map: mapArt
            case .rename: renameArt
            case .menubar: menubarArt
            case .badges: badgesArt
            case .logo: BrandMark(size: 96)
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    private func color(_ k: Int) -> Color { Color(hex: Self.sample[k].1) }

    private static func badge(_ k: Int) -> Badge { Badge(label: sample[k].0, color: String(format: "#%06X", sample[k].1)) }

    /// halo：最高档的光晕（只有侧栏的示意图、讲光晕的那张更新说明画）
    private func avatar(_ k: Int, size: CGFloat, halo: Bool = false) -> some View {
        BadgeAvatar(badge: Self.badge(k), size: size, halo: halo)
    }

    /// 示意用的员工（不是真的账号表里的）：第一个在忙（App 开着），其余照给的状态。
    private static func demo(_ k: Int, plan: String, tier: PlanTier, mood: Employee.Mood) -> Employee {
        let a = Account.sample(["key": sample[k].2, "kind": "oauth", "login": "logged_in", "plan": plan, "plan_tier": tier.rawValue,
                                "badge": ["label": sample[k].0, "color": badge(k).color], "app": ["running": mood == .working]])
        return Employee(account: a, mood: mood, flags: [])
    }

    /// 三个小工位：在用、额度用完在休息、没登录（椅子空着）。静止的一帧。
    private var officeArt: some View {
        let now = Date().timeIntervalSince1970
        let desks: [(caption: String, employee: Employee)] = [
            ("在用", Self.demo(0, plan: "Ultra", tier: .premium, mood: .working)),
            ("额度用完", Self.demo(2, plan: "Pro 100", tier: .advanced, mood: .resting(until: now + 7800))),
            ("没登录", Self.demo(1, plan: "Pro", tier: .standard, mood: .absent)),
        ]
        return HStack(spacing: 12) {
            ForEach(desks, id: \.caption) { d in
                let a = d.employee.account, shape = RoundedRectangle(cornerRadius: 12, style: .continuous)
                VStack(spacing: 6) {
                    DeskScene(employee: d.employee, animate: false, now: now).frame(width: 158)
                    HStack(spacing: 5) {
                        Text(d.caption).font(.caption).foregroundStyle(.secondary)
                        PlanBadge(plan: a.plan ?? "", tier: a.planTier, size: .compact)
                    }
                }
                .padding(8)
                .background(shape.fill(.background))
                .overlay(shape.strokeBorder(Surface.hairline, lineWidth: 0.5))
            }
        }
    }

    /// 一块白底的小卡片
    private func panel<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        content()
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(.background))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Surface.hairline, lineWidth: 0.5))
    }

    private func curve(_ a: CGPoint, _ b: CGPoint) -> Path {
        var p = Path()
        p.move(to: a)
        let mid = a.x + (b.x - a.x) * 0.55
        p.addCurve(to: b, control1: CGPoint(x: mid, y: a.y), control2: CGPoint(x: mid, y: b.y))
        return p
    }

    /// 这台 Mac 分出三条线，各连一个账号（App 图标的母题：一个出口分给几个账号）。
    private var fanArt: some View {
        GeometryReader { g in
            let w = g.size.width, h = g.size.height
            let hub = CGPoint(x: w * 0.3, y: h * 0.5)
            let ends = [CGPoint(x: w * 0.68, y: h * 0.24), CGPoint(x: w * 0.72, y: h * 0.5), CGPoint(x: w * 0.68, y: h * 0.76)]
            ZStack {
                ForEach(0..<3, id: \.self) { k in
                    curve(hub, ends[k]).stroke(color(k), style: StrokeStyle(lineWidth: 3, lineCap: .round))
                }
                Image(systemName: "laptopcomputer")
                    .font(.system(size: 30))
                    .foregroundStyle(.secondary)
                    .padding(10)
                    .background(Circle().fill(.background))
                    .position(hub)
                ForEach(0..<3, id: \.self) { k in
                    avatar(k, size: 34).position(ends[k])
                }
            }
        }
    }

    /// 侧栏的样子：三个账号，名字右边几根小条（Claude 账号多一根 Fable），开着 App 的有绿点。
    private var sidebarArt: some View {
        panel {
            VStack(spacing: 2) {
                // Ultra 一般没有 5 小时的限制，和真的侧栏一样不画 5h 那一根（UsageStack）
                sidebarRow(0, plan: ("Ultra", .premium), sub: "US-B", five: nil, week: 41, fable: 12, running: true, selected: true)
                sidebarRow(1, plan: nil, sub: "未登录 · US-C", five: nil, week: nil, running: false, selected: false)
                sidebarRow(2, plan: ("Pro 100", .advanced), sub: "US-A", five: 35, week: 76, running: false, selected: false)
            }
            .padding(8)
        }
        .frame(width: 330)
    }

    private func sidebarRow(_ k: Int, plan: (String, PlanTier)?, sub: String, five: Double?, week: Double?, fable: Double? = nil,
                            running: Bool, selected: Bool) -> some View {
        HStack(spacing: 9) {
            avatar(k, size: 28, halo: plan?.1 == .premium)
                .overlay(alignment: .bottomTrailing) {
                    if running { RunningDot(size: 9).offset(x: 1, y: 1) }
                }
            VStack(alignment: .leading, spacing: 1) {
                Text(Self.sample[k].2).font(.callout.weight(.medium))
                HStack(spacing: 4) {
                    if let plan {
                        PlanBadge(plan: plan.0, tier: plan.1, size: .compact)
                        Text("·")
                    }
                    Text(sub)
                }
                .font(.caption2)
                .foregroundStyle(.secondary)
            }
            Spacer(minLength: 6)
            if let week {
                VStack(alignment: .trailing, spacing: 4) {
                    if let five { miniBar("5h", five) }
                    miniBar("周", week)
                    if let fable { miniBar("Fable", fable) }
                }
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(selected ? Color.primary.opacity(0.07) : .clear))
    }

    private func miniBar(_ label: String, _ pct: Double) -> some View {
        HStack(spacing: 4) {
            Text(label).font(.system(size: 9, weight: .medium)).foregroundStyle(.secondary)
            Meter(fraction: pct / 100, color: Palette.usage(percent: pct), height: 3).frame(width: 28)
            Text("\(Int(pct))%").font(.system(size: 10, weight: .medium).monospacedDigit())
                .foregroundStyle(UsageLevel(percent: pct) == .normal ? AnyShapeStyle(.primary) : AnyShapeStyle(Palette.usage(percent: pct)))
                .frame(width: 28, alignment: .trailing)
        }
    }

    /// 一行账号和“打开 Claude App”“网页版”“终端”按钮。
    private var openArt: some View {
        panel {
            HStack(spacing: 10) {
                avatar(0, size: 34)
                VStack(alignment: .leading, spacing: 3) {
                    Text("工作").font(.headline)
                    PlanBadge(plan: "Ultra", tier: .premium, size: .compact)
                }
                Spacer(minLength: 16)
                Label("打开 Claude App", systemImage: "macwindow")
                    .font(.callout.weight(.semibold))
                    .foregroundStyle(.white)
                    .fixedSize()
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                    .background(Capsule().fill(Color.accentColor))
                Label("网页版", systemImage: "safari")
                    .font(.callout)
                    .fixedSize()
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .overlay(Capsule().strokeBorder(Surface.hairline, lineWidth: 1))
                Label("终端", systemImage: "terminal")
                    .font(.callout)
                    .fixedSize()
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .overlay(Capsule().strokeBorder(Surface.hairline, lineWidth: 1))
            }
            .padding(14)
        }
        .frame(maxWidth: 540)
        .padding(.horizontal, 24)
    }

    /// 三块额度：5 小时快用完了（红），本周还早（绿），Fable 单独一块（下面写着它最多占本周额度的多少）。
    private var quotaArt: some View {
        HStack(alignment: .top, spacing: 10) {
            miniTile("5 小时", 92, "2 小时后重置")
            miniTile("本周", 41, "周二 16:00 重置")
            miniTile("Fable", 6, "周二 16:00 重置", note: Texts.modelShare("Fable", allowance: 50, used: 3))
        }
        .fixedSize(horizontal: false, vertical: true)   // 三块一样高（按最高的那块），不撑满整幅图
        .padding(.horizontal, 24)
    }

    private func miniTile(_ title: String, _ pct: Double, _ reset: String, note: String? = nil) -> some View {
        let color = Palette.usage(percent: pct)
        return panel {
            VStack(alignment: .leading, spacing: 8) {
                Text(title).font(.callout.weight(.semibold)).foregroundStyle(.secondary)
                HStack(alignment: .firstTextBaseline, spacing: 1) {
                    Text("\(Int(pct))").font(.system(size: 28, weight: .semibold).monospacedDigit())
                    Text("%").font(.system(size: 15, weight: .semibold))
                }
                .foregroundStyle(UsageLevel(percent: pct) == .normal ? AnyShapeStyle(.primary) : AnyShapeStyle(color))
                Meter(fraction: pct / 100, color: color, height: 6)
                // Fable 那句（名字写在句子里）一行放不下：最多两行，三块照样一样高
                Text(note ?? reset).font(.caption).foregroundStyle(.secondary).lineLimit(2).minimumScaleFactor(0.8)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(14)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
    }

    /// 2.9.0-beta.10 的更新：四档套餐徽章，和最高档头像的光晕。
    private var badgesArt: some View {
        VStack(spacing: 20) {
            HStack(spacing: 22) {
                avatar(2, size: 46)
                avatar(1, size: 46)
                avatar(0, size: 46, halo: true)
            }
            HStack(spacing: 8) {
                PlanBadge(plan: "Free", tier: .entry)
                PlanBadge(plan: "Pro", tier: .standard)
                PlanBadge(plan: "Max", tier: .advanced)
                PlanBadge(plan: "Ultra", tier: .premium)
            }
        }
    }

    /// 一条线路（这台 Mac → 节点 → 出口）和检测结果的小标签。
    private var routeArt: some View {
        var stops = [RouteStop(id: 0, mark: .origin, title: "这台 Mac"), RouteStop(id: 2, title: "US-B"),
                     RouteStop(id: 3, mark: .end, title: "203.0.113.7")]
        stops[1].detail = "经路由器"
        stops[2].detail = "美国"
        return VStack(spacing: 18) {
            RouteStrip(stops: stops, color: color(0))
            HStack(spacing: 6) {
                Tag(text: "纯净度 97 · 极度纯净", color: .green)
                Tag(text: Texts.leak(.ok, what: "DNS"), color: .green)
                Tag(text: Texts.leak(.ok, what: "WebRTC"), color: .green)
            }
        }
        .padding(.horizontal, 28)
    }

    /// 分流图的样子：三个账号 → 路由器 → 两个节点；有一条选错了节点（橙色虚线）。
    private var mapArt: some View {
        GeometryReader { g in
            let w = g.size.width, h = g.size.height
            let ys = [h * 0.24, h * 0.5, h * 0.76]
            let acc = w * 0.16, routerL = w * 0.4, routerR = w * 0.58, node = w * 0.8
            let nodeY = [h * 0.36, h * 0.68]
            ZStack {
                ForEach(0..<3, id: \.self) { k in
                    Path { p in p.move(to: CGPoint(x: acc + 18, y: ys[k])); p.addLine(to: CGPoint(x: routerL, y: ys[k])) }
                        .stroke(color(k), style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                }
                curve(CGPoint(x: routerR, y: ys[0]), CGPoint(x: node - 34, y: nodeY[0]))
                    .stroke(color(0), style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                curve(CGPoint(x: routerR, y: ys[1]), CGPoint(x: node - 34, y: nodeY[1]))
                    .stroke(Color.orange, style: StrokeStyle(lineWidth: 2.5, lineCap: .round, dash: [5, 5]))
                curve(CGPoint(x: routerR, y: ys[2]), CGPoint(x: node - 34, y: nodeY[1]))
                    .stroke(color(2), style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(Surface.tile)
                    .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Surface.hairline, lineWidth: 0.5))
                    .frame(width: routerR - routerL, height: h * 0.78)
                    .overlay(alignment: .top) {
                        Label("路由器", systemImage: "wifi.router").font(.caption2.weight(.semibold)).padding(.top, 6)
                    }
                    .position(x: (routerL + routerR) / 2, y: h * 0.5)
                ForEach(0..<3, id: \.self) { k in
                    Capsule().fill(.background)
                        .overlay(Capsule().strokeBorder(Surface.hairline, lineWidth: 0.5))
                        .overlay(Circle().fill(k == 1 ? Color.orange : .green).frame(width: 6, height: 6))
                        .frame(width: 30, height: 16)
                        .position(x: (routerL + routerR) / 2, y: ys[k])
                    avatar(k, size: 30).position(x: acc, y: ys[k])
                }
                ForEach(0..<2, id: \.self) { j in
                    Text(j == 0 ? "US-B" : "US-A")
                        .font(.callout.weight(.semibold))
                        .padding(.horizontal, 12)
                        .padding(.vertical, 7)
                        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(.background))
                        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Surface.hairline, lineWidth: 0.5))
                        .position(x: node, y: nodeY[j])
                }
            }
        }
    }

    /// 改别名（名字变成输入框）和外观按钮。
    private var renameArt: some View {
        VStack(spacing: 18) {
            HStack(spacing: 12) {
                avatar(0, size: 40)
                HStack(spacing: 0) {
                    Text("主号").font(.system(size: 20, weight: .semibold))
                    Rectangle().fill(Color.accentColor).frame(width: 2, height: 22)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 4)
                .frame(width: 180, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(.background))
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Color.accentColor, lineWidth: 1.5))
                Image(systemName: "checkmark").foregroundStyle(.secondary)
                Image(systemName: "xmark").foregroundStyle(.secondary)
            }
            HStack(spacing: 0) {
                ForEach(AppearanceMode.allCases) { m in
                    Label(m.title, systemImage: m.symbol)
                        .font(.caption)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 6)
                        .background(Capsule().fill(m == .dark ? Color.primary.opacity(0.1) : .clear))
                }
            }
            .padding(3)
            .overlay(Capsule().strokeBorder(Surface.hairline, lineWidth: 0.5))
        }
    }

    /// 菜单栏上的 cca 图标和它弹出的面板。
    private var menubarArt: some View {
        VStack(spacing: 0) {
            HStack(spacing: 14) {
                Spacer()
                Image(systemName: "wifi").font(.caption)
                BrandMark(size: 18)
                    .padding(3)
                    .background(RoundedRectangle(cornerRadius: 5).fill(Color.primary.opacity(0.12)))
                Image(systemName: "battery.75percent").font(.caption)
                Text("10:24").font(.caption.monospacedDigit())
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 5)
            .background(Rectangle().fill(.background.opacity(0.7)))
            HStack {
                Spacer()
                panel {
                    VStack(spacing: 8) {
                        menuRow(0, five: 92)
                        menuRow(2, five: 35)
                    }
                    .padding(10)
                }
                .frame(width: 260)
                .padding(.trailing, 60)
                .padding(.top, 8)
            }
            Spacer(minLength: 0)
        }
        .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
    }

    private func menuRow(_ k: Int, five: Double) -> some View {
        HStack(spacing: 8) {
            avatar(k, size: 24)
            Text(Self.sample[k].2).font(.callout.weight(.medium))
            Spacer()
            miniBar("5h", five)
            Text("打开").font(.caption.weight(.medium))
                .padding(.horizontal, 9)
                .padding(.vertical, 4)
                .overlay(Capsule().strokeBorder(Surface.hairline, lineWidth: 1))
        }
    }
}

// MARK: 提示卡片

/// 提示卡片：给新手讲一句这一页怎么看；点“知道了”就不再显示（“帮助”菜单里可以重新显示）。
struct TipCard: View {
    @Environment(Guide.self) private var guide
    let id: String
    var systemImage = "lightbulb"
    let title: String
    let message: String

    var body: some View {
        if !guide.isDismissed(id) {
            let shape = RoundedRectangle(cornerRadius: 14, style: .continuous)
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: systemImage)
                    .font(.title3)
                    .foregroundStyle(.tint)
                    .frame(width: 24)
                VStack(alignment: .leading, spacing: 4) {
                    Text(title).font(.callout.weight(.semibold))
                    Text(message)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .lineSpacing(2)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 8)
                Button("知道了") {
                    withAnimation(.snappy) { guide.dismiss(id) }
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
            }
            .padding(14)
            .background(shape.fill(Color.accentColor.opacity(0.07)))
            .overlay(shape.strokeBorder(Color.accentColor.opacity(0.2), lineWidth: 0.5))
            .accessibilityElement(children: .contain)
        }
    }
}

// MARK: 下一步

/// 给新手的一句话：这个账号现在能不能用、下一步点哪个按钮。只按 cca 给的状态说话，不重算 cca 的规则；最要紧的一条排最前。
struct Advice {
    /// suggest：建议（周结算提醒），不是问题
    enum Level { case ok, suggest, info, warn, bad }

    let level: Level
    let text: String

    /// 取 Diagnosis 里最要紧的一条（顺序只写在 Diagnosis 里，办公室里员工的状态也按它）。
    static func make(_ a: Account, usage u: UsageEntry?, live: LiveSlot?, appLabel: String, now: Double) -> Advice {
        guard let d = Diagnosis.all(a, usage: u, live: live, now: now).first else {
            return Advice(level: .ok, text: a.isAPI ? "一切正常。" : "一切正常，可以打开使用。")
        }
        switch d {
        case .useFirst(let f):
            return Advice(level: .suggest, text: Texts.useFirst(f, now: now) + "。")
        case .loggedOut:
            return Advice(level: .warn, text: "还没登录这个账号：点「登录…」，在终端里按提示登录。")
        case .emailMismatch(let email):
            let who = email.map { "（现在登录的是 \($0)）" } ?? ""
            return Advice(level: .warn, text: "登录的不是这个账号\(who)：点「重新登录…」换回来。")
        case .loginExpired:
            return Advice(level: .warn, text: "登录已失效（比如改过密码）：点「重新登录…」，在终端里按提示登录。")
        case .appMissing:
            return Advice(level: .info, text: "这台 Mac 上没装 \(appLabel)：可以先用「网页版」。")
        case .portClosed:
            return Advice(level: .bad, text: "这个账号的端口连不上：先点「同步路由器」，还不行就检查一下路由器。")
        case .portDrift:
            return Advice(level: .warn, text: "路由器给这个账号选错了节点：点「同步路由器」就能修好。")
        case .exitBad(let r, let region):
            return Advice(level: .bad, text: "出口有问题：\(Texts.healthResult(r, region: region))。点「检测出口」再测一次。")
        case .quota(let windows):
            // 用完的窗口比快用完的要紧
            let w = windows.first(where: \.exhausted) ?? windows[0]
            let name = w.window == .fiveHour ? "5 小时" : "本周"
            if w.exhausted {
                let back = w.resetsEpoch.map { "，" + Texts.backAt(epoch: $0, now: now) } ?? ""
                return Advice(level: .warn, text: "\(name)的额度用完了\(back)。可以先换个账号。")
            }
            let when = w.resetsEpoch.map { "，" + Texts.resetText(epoch: $0, now: now) } ?? ""
            return Advice(level: .warn, text: "\(name)的额度快用完了（\(Texts.percent(w.percent))\(when)）。可以先换个账号。")
        case .outdatedLoginPlan(let old):
            return Advice(level: .info, text: Texts.outdatedLoginPlan(old, current: a.plan) + "：点用量下面的「重新登录…」更新一次。")
        case .lowPurity:
            return Advice(level: .warn, text: "出口 IP 的纯净度偏低，用的时候注意风险。")
        case .leak:
            return Advice(level: .warn, text: "DNS 或 WebRTC 可能泄露，详情看下面的网络出口。")
        case .neverChecked:
            return Advice(level: .info, text: a.isAPI ? "第一次用这个接口？点「检测连接」看看连不连得上。"
                          : "第一次用这个账号？先点「检测出口」，看看出口在哪、纯净度怎么样。")
        }
    }
}

/// 账号页顶上的那句话：图标、颜色跟着严重程度。
struct AdviceLine: View {
    let advice: Advice

    var body: some View {
        let (icon, color): (String, Color) = {
            switch advice.level {
            case .ok: return ("checkmark.circle.fill", .green)
            case .suggest: return ("hourglass.circle.fill", .accentColor)
            case .info: return ("info.circle.fill", .accentColor)
            case .warn: return ("exclamationmark.circle.fill", .orange)
            case .bad: return ("xmark.octagon.fill", .red)
            }
        }()
        let shape = RoundedRectangle(cornerRadius: 12, style: .continuous)
        HStack(alignment: .firstTextBaseline, spacing: 9) {
            Image(systemName: icon).foregroundStyle(color)
            Text(advice.text).foregroundStyle(.primary).fixedSize(horizontal: false, vertical: true)
        }
        .font(.callout)
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(shape.fill(color.opacity(0.08)))
        .accessibilityElement(children: .combine)
    }
}
