import Foundation

/// iPhone 上 cca App 的「更新了什么」（Mac App 的 WhatsNew，CcaKit）：一次更新一条，最新的在前；更新后第一次打开时给没看过的
/// （Guide.startIfNeeded），设置里的「更新说明」能再看。App 的版本号一直是 1.0（构建号是上传的时间，ios/testflight.sh），所以按 n 认先后：
/// 新写的一条 n 比前一条大 1，date 写传到 TestFlight 的那一天（docs/dev/ios.md「新手引导」）
struct ReleaseNote: Equatable, Identifiable {
    struct Point: Equatable {
        let symbol: String
        let text: String
    }

    let n: Int
    /// 哪天的更新（「11 月 28 日」）：卡片上那一行小字
    let date: String
    let title: String
    let points: [Point]
    /// 小图（TutorialCard.Art 的名字；认不出就画 Logo）
    var art = "logo"

    var id: Int { n }
}

enum WhatsNew {
    static let all: [ReleaseNote] = [
        ReleaseNote(n: 2, date: "11 月 28 日", title: "新手教程、提示卡片，提示会读出来", points: [
            .init(symbol: "book", text: "第一次进来时有新手教程，讲各页怎么看；想再看一遍，点设置里的「新手教程」，每次更新了什么在「更新说明」里。"),
            .init(symbol: "lightbulb", text: "总览、账号页、用量统计、折合金额的顶上有「这一页怎么看」：点「知道了」就不再出现，设置里能重新显示。"),
            .init(symbol: "text.bubble", text: "没能接入、新建或结束会话时，底下浮出一句提示，几秒后自己消失；开着旁白（VoiceOver）时会读出来。"),
            .init(symbol: "chart.bar.xaxis", text: "账号页上「每周用量」和「折合金额」两张图的同一周上下对齐；演示模式时每一页都写着「演示模式」。"),
        ], art: "logo"),
        ReleaseNote(n: 1, date: "11 月 28 日", title: "照 Mac 上的 App 改：账号页标出状态，能同时接着几个会话", points: [
            .init(symbol: "person.text.rectangle", text: "账号页名字下面标出工具、套餐和登录状态，再下面一句话说下一步怎么做；底下的标签用数字标出要处理的账号、开着的终端。"),
            .init(symbol: "rectangle.stack", text: "能同时接着几个会话（最多 8 个），在会话列表顶上或终端页的菜单里切换；菜单里也能改成只看或可以操作。"),
            .init(symbol: "plus.rectangle.on.rectangle", text: "会话列表里就能结束远程新建的会话；账号页里能直接用这个账号新建会话。"),
        ], art: "sessions"),
    ]

    /// 打开时给哪几条（最新的在前）、记作看过哪一条（和 Mac App 的 WhatsNew.onLaunch 一个规矩）：总是记下最新的那一条；新手教程还没看过的
    /// 不给（教程里都讲了）；一条都没看过的（从有「更新了什么」以前的版本更新上来的）只给最新的那一条；看过的给比它新的
    static func onLaunch(tutorialDone: Bool, lastSeen: Int?, notes: [ReleaseNote] = all) -> (show: [ReleaseNote], seen: Int?) {
        guard let latest = notes.map(\.n).max() else { return ([], nil) }
        let seen = (lastSeen.map { latest > $0 } ?? true) ? latest : nil
        guard tutorialDone else { return ([], seen) }
        let show = notes.filter { n in lastSeen.map { n.n > $0 } ?? (n.n == latest) }
        return (show.sorted { $0.n > $1.n }, seen)
    }

    /// 设置里的「更新说明」：都给（最新的在前）
    static var history: [ReleaseNote] { all.sorted { $0.n > $1.n } }
}
