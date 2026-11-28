import CcaRemoteKit
import SwiftUI

// 给新手的引导（Mac App 的 Guide.swift）：第一次进来（配对好、进了演示）时的卡片教程（一步一张，左右滑或点按钮翻；设置里的「新手教程」
// 能再看），更新后第一次打开时的「更新了什么」（一次更新一张，内容在 Model/WhatsNew.swift，设置里的「更新说明」能再看），页面顶上的提示卡片
// （讲一句这一页怎么看，点「知道了」就不再出现，设置里能重新显示）。账号页顶上「现在能不能用、下一步做什么」那一句是 Advice（AccountDetailView）

/// 引导的状态：教程看过没有、看过哪一条更新说明、关掉了哪些提示卡片。记在 UserDefaults 里；演示模式只记在内存里（像刚装好：教程没看过、
/// 提示卡片都在），退出演示时丢掉
@MainActor
@Observable
final class Guide {
    private enum Keys {
        static let done = "guide.tutorialDone"
        /// 关掉了哪几张提示卡片（TipCard.ID 的值）
        static let tips = "guide.dismissedTips"
        /// 看过哪一条「更新了什么」（ReleaseNote.n）
        static let whatsNewSeen = "guide.whatsNewSeen"
    }

    /// 正在看的引导卡片：新手教程，或者这几条更新说明（最新的在前）
    var sheet: Sheet?
    /// 现在在第几张（从 0 起）
    var step = 0
    private(set) var tutorialDone = false
    private(set) var dismissedTips: Set<String> = []
    @ObservationIgnored private var whatsNewSeen: Int?
    /// 演示模式：只记在内存里
    @ObservationIgnored private var memory = false
    /// 这一次（打开 App、进出演示以后）已经看过要不要自己弹了
    @ObservationIgnored private var started = false
    /// 自己弹（新手教程、更新了什么）：截图、界面测试时不弹（CcaApp）
    @ObservationIgnored var automatic = true

    enum Sheet: Identifiable, Equatable {
        case tutorial
        case whatsNew([ReleaseNote])

        var id: String {
            if case .tutorial = self { return "tutorial" }
            return "whatsNew"
        }
    }

    /// paired：打开 App 时已经配对过电脑
    init(demo: Bool, paired: Bool) {
        load(demo: demo, paired: paired)
    }

    /// 进、出演示模式（AppModel.switchBackend）：换一份记法，底下那几页出来时再看要不要弹
    func reset(demo: Bool) {
        sheet = nil
        step = 0
        started = false
        load(demo: demo, paired: false)
    }

    private func load(demo: Bool, paired: Bool) {
        memory = demo
        guard !demo else {
            tutorialDone = false
            dismissedTips = []
            whatsNewSeen = nil
            return
        }
        let d = UserDefaults.standard
        // 有引导以前就配对过的（从以前的版本更新上来的）：不再给新手教程，给这一次更新了什么
        if paired, d.object(forKey: Keys.done) == nil, d.object(forKey: Keys.whatsNewSeen) == nil {
            d.set(true, forKey: Keys.done)
        }
        tutorialDone = d.bool(forKey: Keys.done)
        dismissedTips = Set(d.stringArray(forKey: Keys.tips) ?? [])
        whatsNewSeen = d.object(forKey: Keys.whatsNewSeen) as? Int
    }

    private func save(_ value: Any, _ key: String) {
        if !memory { UserDefaults.standard.set(value, forKey: key) }
    }

    /// 底下那几页出来了、上面没有别的弹层时（RootView）：第一次是新手教程（看过、跳过过的不再自动打开）；更新了是没看过的那几条更新说明
    /// （WhatsNew.onLaunch：给了就记作看过）
    func startIfNeeded() {
        guard automatic, !started, sheet == nil else { return }
        started = true
        let r = WhatsNew.onLaunch(tutorialDone: tutorialDone, lastSeen: whatsNewSeen)
        if let seen = r.seen {
            whatsNewSeen = seen
            save(seen, Keys.whatsNewSeen)
        }
        if !tutorialDone {
            openTutorial()
        } else {
            showWhatsNew(r.show)
        }
    }

    func openTutorial(at step: Int = 0) {
        self.step = step
        sheet = .tutorial
    }

    /// 看完或跳过（往下滑掉也算）：记下来，以后不再自动打开
    func finishTutorial() {
        tutorialDone = true
        save(true, Keys.done)
        if sheet == .tutorial { sheet = nil }
    }

    /// 设置里的「更新说明」：都能翻（最新的在前）
    func openWhatsNew() {
        showWhatsNew(WhatsNew.history)
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

    /// 弹层关掉了（按钮、往下滑）：教程记作看过；更新说明收起
    func dismissSheet() {
        if sheet == .tutorial { finishTutorial() } else { sheet = nil }
    }

    func isDismissed(_ tip: TipCard.ID) -> Bool { dismissedTips.contains(tip.rawValue) }

    func dismiss(_ tip: TipCard.ID) {
        dismissedTips.insert(tip.rawValue)
        save(dismissedTips.sorted(), Keys.tips)
    }

    /// 设置里的「重新显示提示卡片」
    func resetTips() {
        dismissedTips = []
        save([String](), Keys.tips)
    }
}

// MARK: 卡片教程

/// 教程的一张卡片（Mac App 的 TutorialCard，讲的是手机上的这几页）：上面一幅小图（用 App 里真实的部件画），下面标题和几条带图标的要点
struct TutorialCard {
    let title: String
    let points: [(String, String)]
    let art: Art

    /// 小图；更新说明（ReleaseNote.art）按名字找，认不出就画 Logo
    enum Art: String { case pair, office, stats, sessions, notify, logo }

    static let all: [TutorialCard] = [
        TutorialCard(title: "在手机上看电脑上的账号和会话", points: [
            ("qrcode.viewfinder", "和电脑上的 cca 配对一次（扫电脑上的二维码），这里就能看到那台电脑上的账号、用量和会话；可以配对好几台电脑。"),
            ("lock.shield", "内容端到端加密：经 Team 的服务器转发，服务器看不到；账号的状态里也没有邮箱。"),
            ("square.grid.2x2", "底下五页和 Mac 上 cca App 的左边一栏一样：总览、用量统计、折合金额、远程会话、共享与同步。"),
        ], art: .pair),
        TutorialCard(title: "总览：哪个在用，哪个额度已用完", points: [
            ("square.grid.2x2", "每台电脑一组，每个账号一张卡片；最上面切换 Claude、ChatGPT、Antigravity，还可以只看一种套餐。"),
            ("desktopcomputer", "图里的屏幕在动，说明这个账号正在用；趴在桌上表示额度已用完，屏幕上显示几点恢复；椅子空着表示未登录，灰色的人表示登录的不是这个账号。"),
            ("hourglass", "右下角的沙漏表示建议先用（本周额度快重置了、还剩不少）；左下角的小票是还能重置额度的次数。"),
            ("hand.tap", "点击一张卡片，进这个账号的页面：额度、每周用量、折合金额，还有它在电脑上的会话。"),
        ], art: .office),
        TutorialCard(title: "用量统计、折合金额", points: [
            ("chart.bar", "「用量统计」把每个账号每周用了多少排在一起：哪个总不够用、哪个常常剩一大半，一眼看出来。"),
            ("dollarsign.circle", "「折合金额」按官方 API 的价格把每个账号用掉的 token 折算成美元，方便比较哪个用得多；订阅并不按此收费。"),
            ("calendar", "折合金额可以按周或按天看；几台电脑上的同一个账号只列一次。"),
        ], art: .stats),
        TutorialCard(title: "远程会话：在手机上接着用", points: [
            ("terminal", "「远程会话」列出电脑上用 cca 启动的会话：点一个接入，可以操作，也可以只看。"),
            ("rectangle.stack", "可以同时接着几个会话（最多 \(AppModel.terminalLimit) 个），在列表顶上或终端页的菜单里切换；返回列表时终端照样接着。"),
            ("plus.circle", "电脑上打开了「允许远程启动」时，可以照电脑上的模板新建会话；远程新建的会话也能在这里结束。"),
            ("hand.pinch", "在终端里双指捏合缩放、手指拖动滚动；键盘上面那一排是 esc、ctrl、tab 和方向键。"),
        ], art: .sessions),
        TutorialCard(title: "通知和设置", points: [
            ("bell.badge", "会话做完了、在等你时会通知你，锁屏时也能收到；账号的额度用完、恢复时也会通知（可以在设置中关闭）。"),
            ("person.3", "「共享与同步」显示电脑在哪个 Team 里；加入、邀请、分配账号都在电脑上操作。"),
            ("gearshape", "右上角的齿轮打开设置：配对的电脑、通知、外观。想再看一遍这个教程，点设置里的「新手教程」；每次更新了什么在「更新说明」里。"),
        ], art: .notify),
    ]
}

/// 卡片的一页（新手教程的一步、更新说明的一张）
struct CardPage {
    let art: TutorialCard.Art
    let caption: String
    let title: String
    let points: [(String, String)]
}

/// 卡片弹层（新手教程、更新说明共用，Mac App 的 CardSheet）：一页一张，上面一幅小图，下面一行小字、标题和几条带图标的要点；左右滑动翻页，
/// 最下面是第几页（PageDots）和按钮。页面太长（字放大了）时每一页自己滚
struct CardSheet<Footer: View>: View {
    /// 读屏念的名字（新手教程、更新说明）
    let name: String
    let pages: [CardPage]
    let step: Binding<Int>
    /// 一页叫什么（新手教程是「步」，更新说明是「张」），读屏念第几页时用
    let unit: String
    @ViewBuilder let footer: (Int) -> Footer

    var body: some View {
        let i = min(max(step.wrappedValue, 0), pages.count - 1)
        VStack(spacing: 0) {
            TabView(selection: step) {
                ForEach(pages.indices, id: \.self) { k in
                    ScrollView { page(pages[k]) }
                        .scrollBounceBehavior(.basedOnSize)
                        .tag(k)
                        .accessibilityElement(children: .contain)
                        .accessibilityLabel("\(name)，\(pages[k].caption)：\(pages[k].title)")
                }
            }
            .tabViewStyle(.page(indexDisplayMode: .never))
            VStack(spacing: 12) {
                if pages.count > 1 {
                    PageDots(count: pages.count, current: i, unit: unit) { k in withAnimation(.snappy) { step.wrappedValue = k } }
                }
                footer(i)
            }
            .controlSize(.large)
            .padding(.horizontal, 20)
            .padding(.top, 8)
            .padding(.bottom, 18)
            .pageColumn()
        }
    }

    private func page(_ p: CardPage) -> some View {
        VStack(alignment: .leading, spacing: 18) {
            TutorialArt(art: p.art)
                .frame(maxWidth: .infinity)
                .frame(height: 188)
                .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Surface.tile))
            VStack(alignment: .leading, spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(p.caption)
                        .font(.caption.weight(.medium).monospacedDigit())
                        .foregroundStyle(.secondary)
                    Text(p.title)
                        .font(.title2.weight(.semibold))
                        .fixedSize(horizontal: false, vertical: true)
                }
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(p.points.indices, id: \.self) { k in
                        HStack(alignment: .firstTextBaseline, spacing: 10) {
                            Image(systemName: p.points[k].0)
                                .font(.callout.weight(.semibold))
                                .foregroundStyle(.tint)
                                .frame(width: 22)
                                .accessibilityHidden(true)
                            Text(p.points[k].1)
                                .font(.callout)
                                .foregroundStyle(.primary.opacity(0.85))
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            }
            .padding(.horizontal, 4)
        }
        .padding(.horizontal, 20)
        .padding(.top, 28)
        .padding(.bottom, 8)
        .pageColumn()
    }
}

/// 引导的弹层（RootView、设置里打开的）：新手教程或者更新说明
struct GuideSheet: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            switch model.guide.sheet {
            case .tutorial?: TutorialSheet()
            case .whatsNew?: WhatsNewSheet()
            case nil: EmptyView()
            }
        }
        .presentationDragIndicator(.visible)
    }
}

/// 卡片教程：一步一张；第一张可以跳过，最后一张能「从头再看」或「开始使用」（往下滑掉也算看过）
private struct TutorialSheet: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let guide = model.guide
        let cards = TutorialCard.all
        let pages = cards.enumerated().map { k, c in
            CardPage(art: c.art, caption: "第 \(k + 1) 步，共 \(cards.count) 步", title: c.title, points: c.points)
        }
        CardSheet(name: "新手教程", pages: pages, step: Binding(get: { guide.step }, set: { guide.step = $0 }), unit: "步") { i in
            let last = i == cards.count - 1
            let next = Button(last ? "开始使用" : "下一步") {
                if last { guide.finishTutorial() } else { withAnimation(.snappy) { guide.step = i + 1 } }
            }
            .buttonStyle(.glassProminent)
            let back = Button("上一步") { withAnimation(.snappy) { guide.step = i - 1 } }.buttonStyle(.glass)
            // 最后一张：从头再看、上一步、开始使用；放不下（字放大了）时不要「上一步」（点上面的点、往右滑也能回去）
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 10) {
                    if last {
                        Button("从头再看") { withAnimation(.snappy) { guide.step = 0 } }.buttonStyle(.glass)
                    } else if i == 0 {
                        Button("跳过") { guide.finishTutorial() }.buttonStyle(.glass)
                    }
                    if i > 0 { back }
                    Spacer(minLength: 0)
                    next
                }
                HStack(spacing: 10) {
                    if last {
                        Button("从头再看") { withAnimation(.snappy) { guide.step = 0 } }.buttonStyle(.glass)
                    } else if i == 0 {
                        Button("跳过") { guide.finishTutorial() }.buttonStyle(.glass)
                    } else {
                        back
                    }
                    Spacer(minLength: 0)
                    next
                }
            }
        }
    }
}

/// 更新了什么：一次更新一张（最新的在前），样子和新手教程一样
private struct WhatsNewSheet: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let guide = model.guide
        let notes = guide.whatsNewNotes
        if !notes.isEmpty {
            let pages = notes.map { n in
                CardPage(art: TutorialCard.Art(rawValue: n.art) ?? .logo, caption: "\(n.date)更新了什么", title: n.title,
                         points: n.points.map { ($0.symbol, $0.text) })
            }
            CardSheet(name: "更新说明", pages: pages, step: Binding(get: { guide.step }, set: { guide.step = $0 }), unit: "张") { i in
                let last = i == notes.count - 1
                HStack(spacing: 10) {
                    if !last {
                        Button("关闭") { guide.dismissSheet() }.buttonStyle(.glass)
                    }
                    if i > 0 {
                        Button("上一张") { withAnimation(.snappy) { guide.step = i - 1 } }.buttonStyle(.glass)
                    }
                    Spacer(minLength: 0)
                    Button(last ? "知道了" : "更早的版本") {
                        if last { guide.dismissSheet() } else { withAnimation(.snappy) { guide.step = i + 1 } }
                    }
                    .buttonStyle(.glassProminent)
                }
            }
        }
    }
}

/// 第几页（Mac App 的 PageDots）：一排点，现在这一页是一根短条，点哪个点就跳到哪一页；超过 maxDots 页只写「3 / 37」
struct PageDots: View {
    static let maxDots = 12
    let count: Int
    let current: Int
    /// 一页叫什么（「步」「张」）
    let unit: String
    let select: (Int) -> Void

    var body: some View {
        if count > Self.maxDots {
            Text("\(current + 1) / \(count)")
                .font(.callout.monospacedDigit())
                .foregroundStyle(.secondary)
                .accessibilityLabel("第 \(current + 1) \(unit)，共 \(count) \(unit)")
        } else {
            HStack(spacing: 0) {
                ForEach(0..<count, id: \.self) { k in
                    Button {
                        select(k)
                    } label: {
                        Capsule()
                            .fill(k == current ? AnyShapeStyle(.primary) : AnyShapeStyle(.quaternary))
                            .frame(width: k == current ? 18 : 7, height: 7)
                            .padding(.vertical, 8)
                            .padding(.horizontal, 4)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("第 \(k + 1) \(unit)")
                    .accessibilityAddTraits(k == current ? .isSelected : [])
                }
            }
            .animation(.snappy(duration: 0.2), value: current)
        }
    }
}

/// 教程卡片上的小图：用 App 里真实的部件画（示意，不能点）。总览那张的工位是演示模式里的账号（静止的一帧）
struct TutorialArt: View {
    let art: TutorialCard.Art

    var body: some View {
        Group {
            switch art {
            case .pair: pairArt
            case .office: officeArt
            case .stats: statsArt
            case .sessions: sessionsArt
            case .notify: notifyArt
            case .logo: BrandMark(size: 92)
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    /// 电脑和手机，中间一把锁（端到端加密）
    private var pairArt: some View {
        HStack(spacing: 12) {
            Image(systemName: "laptopcomputer").font(.system(size: 58, weight: .light))
            HStack(spacing: 6) {
                dashes
                Image(systemName: "lock.fill")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(.tint)
                    .padding(11)
                    .background(Circle().fill(Color.accentColor.opacity(0.12)))
                dashes
            }
            Image(systemName: "iphone").font(.system(size: 50, weight: .light))
        }
        .foregroundStyle(.secondary)
    }

    private var dashes: some View {
        HStack(spacing: 4) {
            ForEach(0..<3, id: \.self) { _ in Capsule().fill(.tertiary).frame(width: 7, height: 3) }
        }
    }

    /// 三个工位：在用、额度用完（趴在桌上，屏幕上写几点恢复）、没登录或登录失效（椅子空着）
    private var officeArt: some View {
        let now = Date().timeIntervalSince1970
        let all = ["a1", "b2"].flatMap { DemoAccounts.status($0, machine: "", now: now)?.accounts ?? [] }.map { ($0, AccountMood($0, now: now)) }
        let picks = [all.first { $0.1.busy }, all.first { !$0.1.busy && Self.usedUp($0.1) },
                     all.first { !$0.1.busy && ($0.1.trouble == .expired || $0.1.trouble == .loggedOut) }].compactMap { $0 }
        return HStack(spacing: 4) {
            ForEach(picks.indices, id: \.self) { k in
                DeskScene(account: picks[k].0, look: DeskLook(picks[k].0, mood: picks[k].1), animate: false, now: now)
            }
        }
        .padding(.horizontal, 10)
    }

    private static func usedUp(_ m: AccountMood) -> Bool {
        if case .usedUp? = m.trouble { return true }
        return false
    }

    /// 每周用量的一排条（填得越满用得越多），下面是折合金额的一排柱子
    private var statsArt: some View {
        let used: [Double] = [58, 74, 112, 81, 66, 47, 95, 70, 39, 88, 77, 63]
        let usd: [Double] = [3.2, 4.1, 5.6, 4.4, 3.9, 2.8, 5.1, 4.0, 2.2, 4.8, 4.5, 3.6]
        return VStack(alignment: .leading, spacing: 10) {
            row("每周用量") {
                ForEach(used.indices, id: \.self) { k in
                    WeekBar(percent: used[k], top: 112, color: Palette.usage(percent: used[k])).frame(width: 9, height: 56)
                }
            }
            row("折合金额") {
                ForEach(usd.indices, id: \.self) { k in
                    CostColumn(usd: usd[k], top: 6, color: CostBarChart.barColor).frame(width: 9, height: 44)
                }
            }
        }
    }

    private func row<Bars: View>(_ label: String, @ViewBuilder bars: () -> Bars) -> some View {
        HStack(alignment: .bottom, spacing: 12) {
            Text(label).font(.caption).foregroundStyle(.secondary).frame(width: 56, alignment: .leading)
            HStack(alignment: .bottom, spacing: 6) { bars() }
        }
    }

    /// 会话列表的一行（已接入）和它的终端
    private var sessionsArt: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                StatusDot(kind: .on)
                Text("重构登录流程").font(.subheadline.weight(.semibold))
                Spacer(minLength: 8)
                Text("已接入").font(.caption).foregroundStyle(.green)
            }
            VStack(alignment: .leading, spacing: 5) {
                Text("› 把登录拆成两步，测试也补上").foregroundStyle(Color(hex: 0xB79CFF))
                Text("● 改好了 3 个文件，测试全部通过").foregroundStyle(.white.opacity(0.86))
                Text("› ▍").foregroundStyle(Color(hex: 0xB79CFF))
            }
            .font(.system(size: 12, design: .monospaced))
            .lineLimit(1)
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color(uiColor: TerminalSurface.background)))
        }
        .padding(.horizontal, 26)
    }

    /// 锁屏上的两条通知：会话做完了、额度恢复了
    private var notifyArt: some View {
        VStack(spacing: 8) {
            banner("Claude Code 做完了", "工作室的 Mac mini · 重构登录流程")
            banner("额度恢复了", "5 小时的额度恢复了，可以接着用了")
        }
        .padding(.horizontal, 22)
    }

    private func banner(_ title: String, _ body: String) -> some View {
        HStack(spacing: 10) {
            BrandMark(size: 22)
                .padding(6)
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color(uiColor: .systemBackground)))
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Text(title).font(.footnote.weight(.semibold)).lineLimit(1)
                    Spacer(minLength: 4)
                    Text("现在").font(.caption2).foregroundStyle(.secondary)
                }
                Text(body).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
        }
        .padding(10)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(.regularMaterial))
    }
}

// MARK: 提示卡片

/// 提示卡片（Mac App 的 TipCard）：给新手讲一句这一页怎么看；点「知道了」就不再显示（设置里的「重新显示提示卡片」让它们回来）
struct TipCard: View {
    /// 哪一张。值记在 UserDefaults 里（关掉了哪几张），不能改：改了，点过「知道了」的又会出来。和 Mac App 的同一页同一个值
    enum ID: String, CaseIterable {
        case office = "office-page", account = "account-page", usageStats = "usage-stats-page", costStats = "cost-stats-page"
    }

    @Environment(AppModel.self) private var model
    let id: ID
    var systemImage = "lightbulb"
    var title = "这一页怎么看"
    let message: String

    var body: some View {
        if !model.guide.isDismissed(id) {
            let shape = RoundedRectangle(cornerRadius: 14, style: .continuous)
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: systemImage)
                    .font(.title3)
                    .foregroundStyle(.tint)
                    .frame(width: 24)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 6) {
                    // 手机窄：「知道了」放在标题那一行的右边，说明占满整个宽度（Mac 上是最右边一栏）
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(title).font(.callout.weight(.semibold)).accessibilityAddTraits(.isHeader)
                        Spacer(minLength: 8)
                        Button("知道了") {
                            withAnimation(.snappy) { model.guide.dismiss(id) }
                        }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                        .accessibilityHint("不再显示这张提示")
                    }
                    Text(message)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .lineSpacing(2)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(shape.fill(Color.accentColor.opacity(0.07)))
            .overlay(shape.strokeBorder(Color.accentColor.opacity(0.2), lineWidth: 0.5))
            .accessibilityElement(children: .contain)
        }
    }
}
