import Foundation

// 每个版本“更新了什么”：App 升级后第一次打开时一张张给你看（和新手教程一样的卡片），“帮助”菜单的「更新说明」能再看。
// 发布前 release.sh 检查这里有没有这个版本的一条（找 `ReleaseNote(version: "版本号"`），没有就不发布——
// 写的时候顺便看看新手教程（Guide.swift 的 TutorialCard）要不要跟着改。界面上的文字照实说，不用办公室、员工这类比喻。

/// 一个版本的更新说明：标题和几条带图标的要点；art 是卡片上那幅小图（CcaApp 认得的名字，认不出就画 Logo）。
struct ReleaseNote: Sendable, Equatable {
    let version: String
    let title: String
    let points: [Point]
    var art: String = ""

    struct Point: Sendable, Equatable {
        let symbol: String
        let text: String
    }
}

enum WhatsNew {
    /// 最新的在前。
    static let all: [ReleaseNote] = [
        ReleaseNote(version: "3.1.0-beta.2", title: "看清还能重置几次", points: [
            .init(symbol: "ticket",
                  text: "ChatGPT 账号有重置机会时，总览卡片上一张小票代表一次，旁边写着最早什么时候过期。"),
            .init(symbol: "arrow.counterclockwise",
                  text: "Claude 账号的右键菜单和「…」里多了「查看重置次数」：用这个账号自己的浏览器和网络出口打开 claude.ai 的用量页，重置次数和到期日都在那里。"),
        ], art: "sidebar"),
        ReleaseNote(version: "3.1.0-beta.1", title: "在账号菜单里查看 API 额度", points: [
            .init(symbol: "creditcard",
                  text: "账号的右键菜单和「…」里多了「查看 API 额度」：用这个账号自己的浏览器和网络出口打开控制台的账单页，订阅附带的每月 API 额度和余额都在那里。第一次打开时要登录控制台。"),
        ], art: "sidebar"),
        ReleaseNote(version: "3.0.0", title: "cca 3.0 正式版", points: [
            .init(symbol: "checkmark.seal",
                  text: "这是 cca 3.0 正式版，包含之前各个测试版的全部内容。以后的正式版会自动更新，不用再手动指定版本号。"),
            .init(symbol: "macwindow",
                  text: "图形界面 App 平时在菜单栏后台运行：添加账号、编辑账号表、配置路由器和本机 Clash 都在窗口里完成，维护者的节点映射也会自动跟上。"),
            .init(symbol: "book",
                  text: "想看每个版本更新了什么：「帮助」菜单里的「更新说明」可以一张张翻。"),
        ], art: "sidebar"),
        ReleaseNote(version: "2.9.0-beta.22", title: "可以马上拉取维护者的映射", points: [
            .init(symbol: "arrow.down.to.line",
                  text: "不用等每小时的自动检查：在「节点分流」页点「拉取映射」，或者在账号的右键菜单和「…」里选「现在拉取映射」，维护者刚发布的改动马上就能拉下来。"),
        ], art: "sidebar"),
        ReleaseNote(version: "2.9.0-beta.21", title: "账号表、路由器和本机 Clash 都在 App 里填", points: [
            .init(symbol: "tablecells",
                  text: "编辑 Claude 和 ChatGPT 的账号表不用再开终端：节点、接口地址、模型、环境变量都在 App 里改，不合格的地方直接写在输入框下面。"),
            .init(symbol: "wifi.router",
                  text: "配置路由器和本机 Clash 也都在 App 里填。路由器的 SSH 密码在 App 里输入，只用这一次，不保存。"),
            .init(symbol: "arrow.triangle.2.circlepath",
                  text: "cca 开着时每小时看一次维护者的节点映射，有变化就自动改好账号表并同步路由器。需要重载路由器时不打扰你，会显示待同步，点「同步」时再问。"),
            .init(symbol: "menubar.rectangle",
                  text: "点「打开主窗口」后，菜单栏的小窗口会自动收起。"),
        ], art: "sidebar"),
        ReleaseNote(version: "2.9.0-beta.20", title: "锁定请求改成一条命令", points: [
            .init(symbol: "lock",
                  text: "「请求维护者锁定节点」现在复制的是一条命令：发给维护者，在 cca 的仓库里运行，确认后就写进映射并发布。请求里不再带别名。"),
        ], art: "sidebar"),
        ReleaseNote(version: "2.9.0-beta.19", title: "平时在后台运行，节点可以请求维护者锁定", points: [
            .init(symbol: "menubar.rectangle",
                  text: "cca 平时在后台运行，只有菜单栏图标，点「打开主窗口」时才出现在 Dock。关掉窗口、在 Dock 里点“退出”、按 ⌘Q 都只是回到后台，用量继续更新，内置代理不停；要彻底退出，点菜单栏面板里的「退出」。"),
            .init(symbol: "terminal",
                  text: "账号页的「终端」用你选的终端，登录、配置路由器这些命令固定用系统自带的终端；系统终端里按任意键后窗口会自动关掉。"),
            .init(symbol: "lock",
                  text: "账号的右键菜单或「…」里可以「请求维护者锁定节点」：生成一行请求并复制，发给维护者，写进映射后这个账号的节点就固定了。"),
        ], art: "sidebar"),
        ReleaseNote(version: "2.9.0-beta.18", title: "终端工具也能检查更新，账号顺序同步到终端", points: [
            .init(symbol: "terminal",
                  text: "「客户端更新」现在也检查终端里的 Claude Code 和 Codex：有新版本时确认一下就能更新，开着的会话不受影响，下次启动时用新版本。"),
            .init(symbol: "arrow.up.arrow.down",
                  text: "在 App 里调整账号顺序或置顶，终端里的菜单和编号也跟着变；在终端里改了账号表的顺序，App 也跟着变。调整顺序不用同步路由器，账号的颜色也不变。"),
        ], art: "sidebar"),
        ReleaseNote(version: "2.9.0-beta.17", title: "账号可以排序和置顶，添加账号在 App 里完成", points: [
            .init(symbol: "pin",
                  text: "账号可以拖动排序（侧栏上下拖，总览里拖到别的卡片上），常用的右键置顶。顺序只影响 App 里的显示。"),
            .init(symbol: "plus",
                  text: "添加账号、第三方 API 账号、更换 API key、删除账号，现在在 App 里一页页填完；侧栏的账号列表上多了「＋」。"),
            .init(symbol: "checkmark.shield",
                  text: "网络出口在后台定时检测：在用的账号半小时左右一次，没在用的几小时一次，你走开或系统过热时暂停。"),
            .init(symbol: "person.badge.key",
                  text: "改过密码导致登录失效时，ChatGPT 账号会直接提示；任何账号都可以在右键菜单或「…」里「重新登录…」。"),
        ], art: "sidebar"),
        ReleaseNote(version: "2.9.0-beta.16", title: "支持更多终端，用量自动刷新", points: [
            .init(symbol: "terminal",
                  text: "「终端」支持 iTerm2、Ghostty、Warp、WezTerm、kitty、Alacritty、Tabby、Rio、Hyper、cmux；Termius、Wave 会打开它并复制命令，粘贴运行。没装的显示为灰色。"),
            .init(symbol: "arrow.clockwise",
                  text: "用量在后台自动刷新：只查到期的账号，在用的查得勤、没在用的很少查；你走开或系统过热时暂停，不和你抢 CPU。"),
            .init(symbol: "arrow.down.app",
                  text: "Claude、ChatGPT 客户端每 6 小时检查一次更新，不再每分钟起一个进程。"),
            .init(symbol: "network",
                  text: "WebRTC 检测放宽：只有看到中国大陆的地址才警告，其他地区只作备注。"),
        ], art: "quota"),
        ReleaseNote(version: "2.9.0-beta.15", title: "在终端里打开账号，Claude App 也能统一更新", points: [
            .init(symbol: "terminal",
                  text: "账号页多了「终端」按钮：在你选的文件夹里用这个账号开命令行，最近用过的文件夹会记住，所有账号共用。"),
            .init(symbol: "macwindow",
                  text: "自动认出 iTerm2、Warp、WezTerm、Ghostty、kitty、Alacritty，在设置或「终端」按钮的下拉菜单里选默认用哪个。"),
            .init(symbol: "arrow.down.app",
                  text: "「客户端更新」现在也管 Claude App：自动检查、下载并校验；关掉 Claude 的全部窗口、你确认后才安装。"),
            .init(symbol: "moon",
                  text: "深色外观下，菜单栏面板和左边列表里的灰字更清楚了。"),
        ], art: "open"),
        ReleaseNote(version: "2.9.0-beta.14", title: "界面更简洁", points: [
            .init(symbol: "sparkle",
                  text: "套餐徽章不再整块上色：都是灰色的小胶囊，档位看前面的小记号（灰圈、蓝点、紫色菱形，最高档是彩色的四角星）。"),
            .init(symbol: "circle.dashed",
                  text: "最高档账号的光晕只留在左边列表的头像上，换成一圈细细的环，也不会再被切掉一块。"),
            .init(symbol: "person.text.rectangle",
                  text: "账号页的头部回到简洁的样子，不套卡片、不加彩色边框；总览的卡片和插画也不再按套餐上色。"),
            .init(symbol: "bolt",
                  text: "从命令行启动账号时少起几次 Python，快一点。"),
        ], art: "badges"),
        ReleaseNote(version: "2.9.0-beta.13", title: "加强账号隔离，集中管理 ChatGPT 更新", points: [
            .init(symbol: "key.horizontal",
                  text: "加强各账号的钥匙串和登录凭证隔离，阻止终端环境或共享设置意外覆盖账号凭证。"),
            .init(symbol: "checkmark.shield",
                  text: "启动前回查实际选中的出口节点；节点切换失败或无法核实时停止启动，避免走错出口。"),
            .init(symbol: "network",
                  text: "收紧浏览器 WebRTC 直连限制；再次打开浏览器或桌面 App 前核对账号资料和出口，不符时要求退出重开。"),
            .init(symbol: "arrow.down.app",
                  text: "新增「客户端更新」页：自动检查、下载并验签 ChatGPT 更新；退出全部实例并确认后才安装，目标变化会停止，意外中断可恢复。"),
        ], art: "open"),
        ReleaseNote(version: "2.9.0-beta.12", title: "加强账号隔离与启动检查", points: [
            .init(symbol: "checkmark.shield",
                  text: "检查共享设置、项目设置和启动参数，发现可能换掉登录或出口的配置时停止启动。"),
            .init(symbol: "macwindow",
                  text: "打开或切回桌面 App 时核验账号资料目录；启动中断时清理能识别的本次新实例。"),
            .init(symbol: "network",
                  text: "网页版和命令行登录页使用账号专属 Chrome；无法按此方式打开时，不再退回默认浏览器。"),
            .init(symbol: "exclamationmark.shield",
                  text: "检测失败会明确显示“没测成”；DNS、WebRTC 的探针结果不再表述为浏览器“无泄露”。"),
        ], art: "open"),
        ReleaseNote(version: "2.9.0-beta.11", title: "Fable 用量、重置额度、检查更新", points: [
            .init(symbol: "gauge.with.dots.needle.33percent",
                  text: "账号页上 Fable 的用量单独一大块：用了多少、什么时候重置，还写着它最多能用本周额度的多少。"),
            .init(symbol: "arrow.counterclockwise",
                  text: "ChatGPT 账号有重置机会时，用量里写着还能重置几次；点「重置额度…」能把 5 小时和每周的用量清零（会先问你）。"),
            .init(symbol: "arrow.down.circle",
                  text: "App 每天自动检查一次新版本，有新版本时窗口顶上会提示；点左下角的版本号或菜单里的「检查更新…」随时查。"),
            .init(symbol: "sparkles",
                  text: "以后每次更新后第一次打开，都会这样告诉你更新了什么；「帮助」菜单里的「更新说明」可以再看。"),
        ], art: "quota"),
        ReleaseNote(version: "2.9.0-beta.10", title: "新的 Logo 和套餐徽章", points: [
            .init(symbol: "point.3.connected.trianglepath.dotted", text: "新的 Logo：一个出口分给几个账号，颜色互相渐变。"),
            .init(symbol: "seal", text: "套餐徽章分四档：灰色圆圈、蓝色圆点、紫色菱形，最高档（Ultra、Pro 500）是彩色的四角星。"),
            .init(symbol: "circle.dashed", text: "左边列表里，最高档账号的头像外面有一圈光晕，账号在用时会慢慢流动。"),
        ], art: "badges"),
        ReleaseNote(version: "2.9.0-beta.9", title: "总览：所有账号一眼看全", points: [
            .init(symbol: "square.grid.2x2", text: "打开 App 先看到总览：每个账号一张卡片，Claude、ChatGPT、第三方 API 账号分开排好。"),
            .init(symbol: "desktopcomputer", text: "图里的屏幕在滚代码、聊天，说明这个账号正在用；额度用完、没登录、登录的不是这个账号，一眼就看得出。"),
            .init(symbol: "sparkle", text: "徽章前面的小记号表示套餐的档位。点一张卡片就进这个账号的页面。"),
        ], art: "office"),
    ]

    /// 打开 App 时要给的更新说明（最新的在前）和要记下的“看过的版本”（nil：不改）：
    /// 第一次装（还没看过新手教程）只看教程，这个版本记作看过；升级了给比上次看过的新、不比现在新的几个版本
    /// （跳过了几个版本就都给；不知道上次看到哪个版本——从有这个功能之前的版本升上来——只给这个版本的）；降级了什么都不给、也不改。
    static func onLaunch(tutorialDone: Bool, lastSeen: String?, current: String,
                         notes: [ReleaseNote] = all) -> (show: [ReleaseNote], seen: String?) {
        guard let cur = AppVersion(current) else { return ([], nil) }
        let floor = lastSeen.flatMap(AppVersion.init)
        let seen = (floor.map { cur > $0 } ?? true) ? current : nil
        guard tutorialDone else { return ([], seen) }
        let show = notes.filter { n in
            guard let v = AppVersion(n.version), v <= cur else { return false }
            return floor.map { v > $0 } ?? (v == cur)
        }
        return (show, seen)
    }

    /// 「更新说明」：不比现在新的都给（最新的在前）；版本号认不出时全给。
    static func history(current: String, notes: [ReleaseNote] = all) -> [ReleaseNote] {
        guard let cur = AppVersion(current) else { return notes }
        return notes.filter { AppVersion($0.version).map { $0 <= cur } ?? false }
    }
}

/// 版本号 X.Y.Z 或 X.Y.Z-beta.N 的先后，和 cca 的 ver_gt 一样：同号的测试版比正式版旧，测试版之间比 beta 后面的数。
struct AppVersion: Comparable, Sendable {
    /// X、Y、Z，再加 beta 后面的数（正式版是 Int.max：同号的测试版都比它旧）
    private let key: [Int]

    init?(_ s: String) {
        func number(_ t: some StringProtocol) -> Int? {
            t.isEmpty || !t.allSatisfy({ $0.isASCII && $0.isNumber }) ? nil : Int(t)
        }
        let parts = s.components(separatedBy: "-beta.")
        let nums = parts[0].split(separator: ".", omittingEmptySubsequences: false).map(number)
        guard parts.count <= 2, nums.count == 3, !nums.contains(nil) else { return nil }
        let beta = parts.count == 2 ? number(parts[1]) : Int.max
        guard let beta else { return nil }
        key = nums.compactMap { $0 } + [beta]
    }

    var isBeta: Bool { key[3] != Int.max }

    static func < (a: AppVersion, b: AppVersion) -> Bool { a.key.lexicographicallyPrecedes(b.key) }
}
