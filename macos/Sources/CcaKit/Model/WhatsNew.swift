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
            .init(symbol: "seal", text: "套餐徽章分四档：灰色圆圈、蓝色圆点、紫色菱形，最高档（Ultra、Pro 500）是整块渐变。"),
            .init(symbol: "circle.dashed", text: "最高档的头像有一圈光晕，账号在用时会慢慢流动。"),
        ], art: "badges"),
        ReleaseNote(version: "2.9.0-beta.9", title: "总览：所有账号一眼看全", points: [
            .init(symbol: "square.grid.2x2", text: "打开 App 先看到总览：每个账号一张卡片，Claude、ChatGPT、第三方 API 账号分开排好。"),
            .init(symbol: "desktopcomputer", text: "图里的屏幕在滚代码、聊天，说明这个账号正在用；额度用完、没登录、登录的不是这个账号，一眼就看得出。"),
            .init(symbol: "sparkle", text: "套餐越高徽章越亮。点一张卡片就进这个账号的页面。"),
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
