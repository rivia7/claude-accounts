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
        ReleaseNote(version: "3.1.0-beta.18", title: "直连不通的服务器上，装好就能用", points: [
            .init(symbol: "server.rack",
                  text: "在连不上 claude.ai 的 Linux 服务器上（比如国内的云服务器）安装 cca，不用再配置路由器：cca 直接用内置代理，给出一行命令请维护者运行。维护者运行以后，cca 自己导入节点、配置好代理、装上 Claude Code，映射里的账号自动加进来。"),
            .init(symbol: "key",
                  text: "配置维护者的映射时，cca 给出的是一整行可以直接运行的命令（这台机器的公钥和名字都写在里面），Mac 上顺便复制到剪贴板。维护者加好以后，cca 每 5 秒自己拉一次映射，不用再按回车。"),
        ], art: "route"),
        ReleaseNote(version: "3.1.0-beta.17", title: "没装的命令可以帮你装上，也支持 Antigravity 命令行了", points: [
            .init(symbol: "arrow.down.circle",
                  text: "没装 claude、codex 命令时，cca 会问要不要用官方安装脚本装上（也可以运行 cca install-cli、cca codex install-cli）；账号页“没装 X 命令”旁边有「安装 X 命令…」。先直连，直连装不上时问过你，再经内置代理的一个节点装（用哪个节点由维护者在映射里指定），装完就停。"),
            .init(symbol: "terminal",
                  text: "支持 Google 的 Antigravity 命令行（agy）：在终端里运行 cca agy add 添加账号，每个账号有自己的登录、装机标识和网络出口，会话、记忆照样共用。用过以后左边多一组 Antigravity 账号，能打开终端、网页版、检测出口（没有用量和桌面 App）。"),
            .init(symbol: "lock.shield",
                  text: "cca 启动的 Claude Code 只把这个账号自己的目录加进工作目录，不再是整个 ~/.cca：读账号表、配置、别的账号的目录之前，照你的权限模式先问。登录凭证、节点密钥这些照旧一律不让读。"),
        ], art: "open"),
        ReleaseNote(version: "3.1.0-beta.16", title: "配置备份不再带节点密钥，缓存不再越积越多", points: [
            .init(symbol: "archivebox",
                  text: "一键还原（cca restore）打的配置备份里，不再有维护者的参考节点（里面有节点的密钥），拉取映射时没清掉的临时文件也不打包。你自己的 Clash Verge 配置备份照样在里面。"),
            .init(symbol: "internaldrive",
                  text: "cca 给内置的 Python 代码存的编译缓存每个版本一份（约 0.5 MB），以前从不清理。现在写新的一份时，顺手删掉 7 天前的旧版本。Mac 上系统本来会清，Linux 服务器上以前会一直留着。"),
            .init(symbol: "stop.circle",
                  text: "Linux 服务器上没装 procps 时（比如 Debian 的精简安装），查用量途中按 Ctrl-C 也能马上停下后台的查询，不用等它们自己超时。"),
        ]),
        ReleaseNote(version: "3.1.0-beta.15", title: "在服务器上登录更省事，服务器重启后代理自己起来", points: [
            .init(symbol: "key",
                  text: "在服务器上登录账号时，这台 Mac 上不用先加好同一个账号：运行服务器给的那行 cca login-page，还没有这个账号会问要不要用同一个节点加上它。两边节点的名字不一样也认得出（按节点的地址认）。"),
            .init(symbol: "server.rack",
                  text: "Linux 服务器重启以后，cca 内置的代理自己起来，不用等下次启动账号：配置时问一次，同意就在你自己的 crontab 里加一行（cca local off 时删掉）。Linux 桌面上装了 Clash Verge 的，和 Mac 上一样走它。服务器上的启动横幅不再写 WebRTC 那一行（服务器上没有浏览器，只测 DNS）。"),
            .init(symbol: "checkmark.shield",
                  text: "「检测出口」时 DNS 泄露多等一会儿（最多 20 秒），慢的线路上不再常常写“没测成”。cca restore 先确认备份包能完整读出来才删 ~/.cca，备份不成就什么都不删。"),
        ], art: "route"),
        ReleaseNote(version: "3.1.0-beta.14", title: "5 小时用量又显示出来了，cca 也能装在 Linux 服务器上", points: [
            .init(symbol: "chart.bar",
                  text: "修正上两个版本的问题：Max 账号和部分 Ultra 账号明明有 5 小时限制，5 小时用量却不显示了。那两个版本看错了查到的用量里的一个标记，5 小时用得比本周或 Fable 少时就当成没有 5 小时限制。现在只有查到的用量里根本没有 5 小时这一项（真正没有 5 小时限制的套餐）才不显示。"),
            .init(symbol: "server.rack",
                  text: "cca 能装在 Linux 服务器上了（命令行）：Ubuntu、Debian、Rocky、Fedora、Alpine 等都测过，缺什么安装时会说，并给出装它的那一行命令。连不上路由器的服务器用 cca 内置的代理；API key 存成只有你能读的文件。"),
            .init(symbol: "key",
                  text: "在服务器上登录账号：服务器给出一行 cca login-page 命令，在这台 Mac 上运行它，登录页就用这个账号的 Chrome、经同一个节点打开；登录完验证码已经复制好，贴到服务器上就行。"),
        ], art: "sidebar"),
        ReleaseNote(version: "3.1.0-beta.13", title: "额度用完时，总览卡片上趴在桌上的样子更自然", points: [
            .init(symbol: "moon.zzz",
                  text: "额度用完的账号，卡片上的人弓着背趴在桌上、头枕在胳膊上，zZ 从头顶飘起来。以前头落在身子前下方，中间还露出一截胳膊，看着像头掉了。新手教程里的图也一样。"),
            .init(symbol: "cup.and.saucer",
                  text: "浅色外观下，桌上的杯子看得清了；热气只在空闲（端着杯子）时冒，在用时小臂压着杯口就不冒了。以前杯子和卡片底色几乎一样，热气像几个飘着的灰点。端口不通、选错节点时冒的汗也更清楚。"),
        ], art: "office"),
        ReleaseNote(version: "3.1.0-beta.12", title: "没有 5 小时限制的账号，不再显示 5 小时用量", points: [
            .init(symbol: "chart.bar",
                  text: "Claude Ultra 这类没有 5 小时限制的套餐，左边的账号列表、总览卡片、菜单栏里不再画 5 小时那一根，账号页的用量里也没有「5 小时」那一块。ChatGPT 的 Pro 500 也一样。"),
            .init(symbol: "checkmark.seal",
                  text: "看的是查到的用量里 5 小时限制有没有生效，不按套餐名猜：5 小时限制还在的账号照常显示。以前这类账号的 5 小时一直写着 0%，终端的 cca usage 里现在这一格写「—」。"),
        ], art: "sidebar"),
        ReleaseNote(version: "3.1.0-beta.11", title: "用完的终端、App 不再留在程序坞里", points: [
            .init(symbol: "terminal",
                  text: "在 App 里经“终端”跑完的命令，按任意键后关掉窗口；“终端”没有别的窗口了就退出它，程序坞的「最近使用的 App」里也不再留着它。你自己还开着别的“终端”窗口时不动它。"),
            .init(symbol: "dock.rectangle",
                  text: "cca 打开的账号 Chrome、Claude App、ChatGPT App 完全退出以后，程序坞里留下的图标也会去掉：几秒后程序坞刷新一次（图标栏闪一下）。还开着的、固定在程序坞上的 App 不受影响。"),
            .init(symbol: "switch.2",
                  text: "不想要的话，在 ~/.cca/config.json 里把 dock_tidy 设成 false；系统设置里关掉「在程序坞中显示最近使用的 App」也就不用整理了。"),
        ], art: "open"),
        ReleaseNote(version: "3.1.0-beta.10", title: "每周用量的颜色和额度一样，Fable 的提示更好懂", points: [
            .init(symbol: "chart.bar.xaxis",
                  text: "每周用量的竖条按那一周用了多少上色，和上面的额度一样：70% 以下绿，70–89% 橙，90% 以上红。以前用的是账号的颜色，红色、绿色的账号容易看错（用了 51% 也是红的）。「用量统计」页也一样。"),
            .init(symbol: "text.bubble",
                  text: "Fable 下面那句话改得更直白：「Fable 最多占本周额度的 50%，已占 17%」。Fable 用的是本周额度的一部分，它那份用完了时写「Fable 的份额用完了，别的模型还能用本周剩下的 49%」：账号没用完，换别的模型还能接着用。"),
        ], art: "quota"),
        ReleaseNote(version: "3.1.0-beta.9", title: "随时看到 Fable 用了多少", points: [
            .init(symbol: "chart.bar",
                  text: "左边的账号列表里，Claude 账号名字右边的小条多了一根 Fable：它自己那份每周额度用了多少，快用完时变橙、变红。菜单栏图标里、总览的卡片上也一样有。"),
            .init(symbol: "cursorarrow.rays",
                  text: "鼠标指着 Fable 那一行，会说它最多能用本周额度的多少、已经用掉多少。"),
            .init(symbol: "square.grid.2x2",
                  text: "总览里同一排的卡片一样高，下边对齐。"),
        ], art: "sidebar"),
        ReleaseNote(version: "3.1.0-beta.8", title: "看看每个账号每周用了多少", points: [
            .init(symbol: "chart.bar.xaxis",
                  text: "账号页多了「每周用量」：最近 12 周，每周用了那一周额度的多少。一周一根竖条，填满就是用完；鼠标指着一周看细节，下面一行是平均每周用到多少、几周用完、几周没用到一半。"),
            .init(symbol: "chart.bar.doc.horizontal",
                  text: "左边新增「用量统计」：所有 Claude、ChatGPT 账号的每周用量排在一起，哪个总不够用、哪个常常剩一大半，一眼看出来。点一行打开这个账号。"),
            .init(symbol: "calendar.badge.clock",
                  text: "Claude、OpenAI 都不给以前的用量，所以 cca 每次查用量时记一笔，从这个版本开始一周一周攒起来（刚升级时只有本周）。一周结束前一天没查到的，那根条填成灰色：实际可能更多。"),
            .init(symbol: "text.alignleft",
                  text: "账号页信息行右边的按钮（重新登录、在 claude.ai 查看、复制……）图标和文字靠左，上下排成一列。"),
        ], art: "quota"),
        ReleaseNote(version: "3.1.0-beta.7", title: "本周额度快重置时提醒先用，API key 记在账号页", points: [
            .init(symbol: "hourglass",
                  text: "本周额度 24 小时内就要重置、还剩至少 20% 的账号，名字旁边有个蓝色的沙漏，账号页写着还剩多少、几点重置：建议先用它，别让额度白白作废。账号用不了（没登录、登错号、出口有问题）或者 5 小时额度快用完时不提示。"),
            .init(symbol: "key",
                  text: "账号页多了 API key 一行：第三方 API 账号的 key 在这里，官方账号也可以记一把备查的（比如在控制台里建的，cca 不会拿它登录或启动）。平时只显示开头和结尾几个字，点眼睛看完整的，30 秒后自动收起；「复制」后剪贴板 60 秒自动清掉。"),
            .init(symbol: "tablecells",
                  text: "cca usage 的表格一个账号一行，不再折行：终端窄时先收起邮箱、套餐和重置时间，备注放不下的写成「…」。"),
            .init(symbol: "rectangle.on.rectangle",
                  text: "账号页、节点分流页、客户端更新页的按钮换成统一的样子，同一节里的按钮对齐；「客户端更新」换了和 cca 一样风格的图标，界面上的说明也改得更短、更好懂。"),
        ], art: "quota"),
        ReleaseNote(version: "3.1.0-beta.6", title: "Claude Code 显示的套餐不对时，提示重新登录", points: [
            .init(symbol: "person.badge.key",
                  text: "升级、降级套餐以后，Claude Code 里显示的套餐（启动时的横幅、/status）可能还是旧的：它只在登录时记下套餐，之后不会自己更新。cca 查用量时发现和账号现在的套餐对不上，账号页会说一句，用量下面有「重新登录…」，重新登录一次就会更新。"),
            .init(symbol: "terminal",
                  text: "命令行里也会提示：cca usage 的表格下面、启动这个账号时的横幅里各说一句。额度一直照账号实际的套餐算，不受影响。"),
        ], art: "badges"),
        ReleaseNote(version: "3.1.0-beta.5", title: "看看账号在哪些设备上用", points: [
            .init(symbol: "desktopcomputer",
                  text: "维护者映射里的账号，账号页最下面多了「设备」：哪几台 Mac 正在用它、开着但闲置了多久、离线的最后一次回报在什么时候、上次什么时候用。维护者配置了设备统计的服务器之后才有。"),
            .init(symbol: "lock.shield",
                  text: "经 cca 用映射里的账号时（命令行会话、cca 打开的 App），这台 Mac 会在后台把开着的会话报告给维护者的服务器：只报账号的摘要，看不到邮箱，也不经账号的网络出口。不想报告：在 ~/.cca/config.json 里加上 \"presence\": false。"),
        ], art: "office"),
        ReleaseNote(version: "3.1.0-beta.4", title: "路由器还在用旧配置时，重载一次就好", points: [
            .init(symbol: "arrow.triangle.2.circlepath",
                  text: "路由器上的 OpenClash / ShellCrash 还在按旧配置运行、切不到账号的节点时，cca 会说明原因，问过你再重载一次；以前这种情况每次同步都会失败，这台 Mac 上的账号都启动不了。"),
            .init(symbol: "terminal",
                  text: "在终端里做的操作（登录、添加账号等）没成功时，最后一行会正确显示退出码，不再是一串问号。"),
        ], art: "route"),
        ReleaseNote(version: "3.1.0-beta.3", title: "在账号页直接查看额度和重置次数", points: [
            .init(symbol: "hand.tap",
                  text: "账号页“用量”下面多了「查看重置次数」（Claude 账号）和「查看 API 额度」两个按钮，不用再去右键菜单里找。"),
            .init(symbol: "doc.on.clipboard",
                  text: "打开这两个页面时，这个账号的邮箱会先复制好：网页要登录的话直接粘贴，免得在这个账号的浏览器里登错号。"),
        ], art: "sidebar"),
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
