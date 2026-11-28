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
        ReleaseNote(n: 2, date: "11 月 28 日", title: "账号的问题、电脑上的提示和 Mac 上的一样", points: [
            .init(symbol: "exclamationmark.triangle", text: "总览和账号页照 Mac 说账号的问题和下一步：出口有问题、端口无法连接、节点选错了；还能按 Team 筛。"),
            .init(symbol: "desktopcomputer", text: "电脑上 cca App 页顶的提示手机上也有：cca 有新版本、账号表或共享设置有问题、刷新用量要你同意或者没成功、内置代理停了。"),
            .init(symbol: "pause.circle", text: "电脑睡着了、会话被挂起时说「无响应」，和离线分开（Team 的服务器更新后才有）。"),
            .init(symbol: "book", text: "有了新手教程和各页顶上的「这一页怎么看」，设置里能再看。"),
            .init(symbol: "text.bubble", text: "没能接入、新建或结束会话时，底下浮出一句提示，开着旁白时会读出来。"),
        ], art: "office"),
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
