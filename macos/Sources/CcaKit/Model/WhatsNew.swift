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
        ReleaseNote(version: "3.7.10", title: "远程新建会话时再选账号，会话能拖动排序", points: [
            .init(symbol: "person.crop.circle.badge.checkmark",
                  text: "远程会话的模板不再带账号：在 iPhone 上的 cca App 或另一台 Mac 上照模板新建会话时，再选用这台 Mac 上的哪个账号（终端里是 cca remote launch --account）。放模板的 Mac 要更新到这一版，iPhone 上的 App 也要更新。"),
            .init(symbol: "arrow.up.arrow.down",
                  text: "「远程会话」页里，一台电脑的会话可以拖动调整先后；iPhone 上点「排序」再拖。排好的先后记在这台设备上。"),
            .init(symbol: "server.rack",
                  text: "一台 Mac 最多可以同时开 100 个远程会话（以前是 8 个），App 里也能同时开 100 个终端。Team 的服务器要更新到这一版（建 Team 的设备点「更新服务器…」）才放开。"),
        ]),
        ReleaseNote(version: "3.7.9", title: "手机上也能看总览、用量统计和折合金额", points: [
            .init(symbol: "iphone",
                  text: "iPhone 上的 cca App 照 Mac 的侧栏分成五页：总览、用量统计、折合金额、远程会话、共享与同步。总览里每个账号一张卡片，和 Mac 上的一样。"),
            .init(symbol: "dollarsign.circle",
                  text: "手机上也能看每个账号的折合金额（按周、按天，按模型、按设备）；用量统计把几台 Mac 上的账号放在一起比；「共享与同步」显示每台 Mac 在 Team 里的状态。"),
            .init(symbol: "arrow.down.circle",
                  text: "Mac 上的 cca 要更新到这一版，才会把这些交给配对的手机；iPhone 上的 App 也要更新。Team 的服务器不用更新。"),
        ]),
        ReleaseNote(version: "3.7.8", title: "其他设备也能看到 Antigravity 的套餐，App 的说法更简短", points: [
            .init(symbol: "arrow.triangle.2.circlepath",
                  text: "在一台设备上用 cca 打开过 Antigravity 会话后，Team 里登录了同一账号的其他设备也会显示它的套餐。两台设备都需要更新到这一版，Team 的服务器不用更新。"),
            .init(symbol: "textformat",
                  text: "App 里的说法改得更简短统一：状态用「未登录」「无法连接」「额度已用完」这类短词，出错写「…失败」；套餐只用原名（Pro、Max、Ultra……），不加中文的档位叫法。"),
        ]),
        ReleaseNote(version: "3.7.7", title: "电脑上也能远程新建会话，额度改回由 cca 查", points: [
            .init(symbol: "laptopcomputer.and.iphone",
                  text: "配对的电脑也能远程启动：在「远程会话」页给它打开「允许远程启动」，那台 Mac 上的 cca App 在「可以接入的会话」里点「新建会话…」，照这台的模板新建会话（终端里是 cca remote launch）。两台都要更新到这一版。"),
            .init(symbol: "iphone",
                  text: "iPhone 上的 cca App 一接上远程会话就照手机的大小排，不用先在手机上打字；回到电脑上打一个键又照电脑排。要这一版以后开的会话。"),
            .init(symbol: "gauge.with.dots.needle.33percent",
                  text: "Claude、Antigravity 的额度改回由 cca 去查，又能点「刷新用量」；Claude Code 输入框下面那一行不再由 cca 写。经 cca 运行 claude -p 这类打印模式也不再拦着。"),
        ]),
        ReleaseNote(version: "3.7.6", title: "手机上新建会话，Codex、Antigravity 也会通知你", points: [
            .init(symbol: "plus.rectangle.on.rectangle",
                  text: "在「远程会话」页存好模板（账号、文件夹、参数），给配对的手机打开「允许远程启动」，iPhone 上的 cca App 就能照模板新建会话（App 要更新）。"),
            .init(symbol: "bell.badge",
                  text: "cca 启动的 Codex、Antigravity 会话做完一轮、在等你时也会通知：推到配对的手机，这台 Mac 上弹系统通知。"),
            .init(symbol: "gauge.with.dots.needle.33percent",
                  text: "Claude、Antigravity 的额度改成在 cca 启动的会话里记下（Antigravity 的套餐也是），不再另外去查；Claude Code 输入框下面那一行写账号和用量。"),
            .init(symbol: "chart.pie",
                  text: "折合金额多了缓存命中率：账号页图下面那一行、按模型的列表和折合金额页的合计。"),
        ]),
        ReleaseNote(version: "3.7.5", title: "手机上看账号的额度，用完、恢复时通知你", points: [
            .init(symbol: "person.crop.rectangle.stack",
                  text: "iPhone 上的 cca App 多了「账号」页：配对过的每台 Mac 上有哪些账号、现在怎样，5 小时和本周的额度，每周的用量和这个账号的会话。"),
            .init(symbol: "bell.badge",
                  text: "账号的 5 小时或本周额度用完了通知一次，到了重置的时间再通知一次：手机经推送，锁着屏也收得到；Mac 上的 App 弹系统通知。两边的设置里都能关。"),
            .init(symbol: "server.rack",
                  text: "账号的状态在 Mac 上加密好再交给 Team 的服务器，服务器只经手密文。服务器要更新到这一版（建 Team 的设备点「更新服务器…」），iPhone 上的 App 也要更新。"),
        ]),
        ReleaseNote(version: "3.7.4", title: "手机接上远程会话，电脑上的不再变窄", points: [
            .init(symbol: "keyboard",
                  text: "远程会话照最后打字的那一处排：iPhone 上的 cca App 只是看着时，电脑上的大小不变；在手机上打字才照手机的宽度。"),
            .init(symbol: "hand.pinch",
                  text: "会话比手机屏幕宽时，App 里缩小着显示，双指放大、拖着看。要这一版以后开的会话，App 也要更新。"),
            .init(symbol: "bell.badge",
                  text: "App 的设置里多了「关掉的会话也推送」，默认开着；关掉后只推 App 里开着的那个会话。Team 的服务器要更新到这一版才认它。"),
        ]),
        ReleaseNote(version: "3.7.3", title: "远程会话做完了、在等你时会通知你", points: [
            .init(symbol: "bell.badge",
                  text: "开着远程会话时，Claude Code 做完一轮、要你确认或者在等你，cca 自己会发通知给接着这个会话的设备，不用再自己写钩子。"),
            .init(symbol: "iphone",
                  text: "iPhone 上的 cca App 更新后也弹这些通知；Team 的服务器开了推送时，App 没开着、锁着屏也收得到，内容只在手机上解开。"),
            .init(symbol: "server.rack",
                  text: "Team 的服务器可以接上推送网关（cca team deploy 的 --push、--push-pin），服务器和网关只经手密文。"),
        ]),
        ReleaseNote(version: "3.7.2", title: "折合金额算上 Antigravity", points: [
            .init(symbol: "dollarsign.circle",
                  text: "用 cca 启动的 Antigravity 会话也折合成金额：Gemini 模型按 Gemini API 的标价，Claude 模型按 Anthropic 的价格。"),
            .init(symbol: "person.crop.circle.badge.questionmark",
                  text: "直接运行 agy 的会话、这一版以前的会话认不出是哪个账号的，算进「认不出账号」。"),
            .init(symbol: "arrow.triangle.2.circlepath",
                  text: "Team 给的 Antigravity 账号也在各台设备之间同步；别的设备更新到这一版后，才算得上它们那边的。"),
        ]),
        ReleaseNote(version: "3.7.1", title: "Team 的配置马上生效，服务器要更新会提示", points: [
            .init(symbol: "arrow.triangle.2.circlepath",
                  text: "后台同步到 Team 的新配置后，马上改账号表、同步路由器和内置代理；只是换了节点时直接切过去，正在用的连接不断。"),
            .init(symbol: "server.rack",
                  text: "Team 的服务器要更新时，「共享与同步」里这个 Team 下面会提示；建 Team 的设备点「更新服务器…」就行。"),
            .init(symbol: "text.bubble",
                  text: "新写进 OpenClash 配置的节点要重启 OpenClash 才用得上：现在照实说，不再说“未能核实目标节点”。"),
        ]),
        ReleaseNote(version: "3.7.0", title: "在 iPhone 上接入远程会话", points: [
            .init(symbol: "iphone",
                  text: "「和手机配对」显示的二维码改给 iPhone 上的 cca App 扫，扫了就配好，不用再输入配对码。"),
            .init(symbol: "lock.shield",
                  text: "手机和这台 Mac 一样直接连 Team 的服务器，终端的内容照旧两头加密。"),
            .init(symbol: "server.rack",
                  text: "Team 的服务器要更新到这一版才认 iPhone 上的 App：管理员在 Team 的「更多」里点「更新服务器、搬到别的服务器…」。"),
        ]),
        ReleaseNote(version: "3.6.16", title: "远程会话更好用，Team 可以有几位管理员", points: [
            .init(symbol: "person.2.badge.key",
                  text: "Team 可以有几位管理员：在 Team 的设备列表里「设为管理员」，每位用自己的密钥，随时能收回。"),
            .init(symbol: "list.bullet.rectangle",
                  text: "远程会话列表标出会话属于哪个 Team；账号名按这台机器上的别名显示。"),
            .init(symbol: "bell.badge",
                  text: "远程会话里的通知也推到接着它的那台机器上。"),
            .init(symbol: "doc.on.clipboard",
                  text: "在终端、手机网页里接远程会话时，会话里复制的字也进这台设备的剪贴板；网页里选中即复制。"),
            .init(symbol: "desktopcomputer",
                  text: "总览里别的设备正在用的账号也显示在用，写出是哪台。"),
        ]),
        ReleaseNote(version: "3.6.15", title: "Codex、Antigravity 的账号也有这些功能了", points: [
            .init(symbol: "calendar.badge.clock",
                  text: "Antigravity 也有周结算提醒：本周额度快重置、还剩不少时，提醒先用这个账号。"),
            .init(symbol: "exclamationmark.bubble",
                  text: "Codex、Antigravity 的共享设置有问题时，说清是哪份文件、哪几项、会怎样、怎么改。"),
            .init(symbol: "arrow.down.circle",
                  text: "客户端更新里加上了终端里的 Antigravity，有新版本时能直接更新。"),
            .init(symbol: "square.and.arrow.down",
                  text: "没装 ChatGPT App 时，点「去下载」直接下载官方安装包。"),
            .init(symbol: "person.badge.plus",
                  text: "配置向导里添加账号时，可以选加 Codex 或 Antigravity 的账号。"),
        ]),
        ReleaseNote(version: "3.6.14", title: "Team 的配置能导出、导入、修改", points: [
            .init(symbol: "square.and.arrow.up.on.square",
                  text: "管理员能把 Team 的配置导出成文件、改好再导入，也能直接在终端里改（App 里在 Team 的「更多」菜单）；导入前先列出会改什么，同意了才换。"),
            .init(symbol: "chart.bar.xaxis",
                  text: "折合金额：新模型还没有价格时按同系列上一个版本估算（带「≈」），不再漏算。"),
            .init(symbol: "battery.25",
                  text: "总览里额度快用完的账号，卡片上的人会犯困：点头，头顶的电池一闪一闪。"),
            .init(symbol: "doc.on.clipboard",
                  text: "远程会话的终端里选中文字就复制，右键粘贴。"),
            .init(symbol: "arrow.triangle.branch",
                  text: "在家改了账号表、Team 的配置，内置代理也马上更新本机 Clash 的端口，「节点分流」不再一直提示还没更新。"),
        ]),
        ReleaseNote(version: "3.6.13", title: "Team 的同步服务器不再需要 Python", points: [
            .init(symbol: "shippingbox",
                  text: "部署到自己服务器上的同步服务器换成 cca 自带的程序：systemd 方式不再要 python3，Docker 方式也不再拉 python 镜像，部署时传过去约 10 MB。"),
            .init(symbol: "arrow.uturn.backward.circle",
                  text: "更新服务器时新的起不来，或者中途断开了，自动换回原来的程序和设置，原来的服务照常运行。"),
            .init(symbol: "arrow.triangle.2.circlepath",
                  text: "已经部署的服务器照常能用，管理员「更新服务器」一次就换成新的。托管平台（cca team hosted）只剩 Railway。"),
        ]),
        ReleaseNote(version: "3.6.12", title: "Antigravity 账号页不再显示英文", points: [
            .init(symbol: "text.bubble",
                  text: "额度的组名改成中文（Gemini 模型；Claude、GPT 模型），agy 给的英文说明不再显示，认不出时间窗口的额度叫「其他额度」。"),
            .init(symbol: "rectangle.compress.vertical",
                  text: "一组只有一个额度、没有百分比时，「不可用」或「还剩 N 次」直接写在组名后面，不再另起一行重复组名。"),
        ]),
        ReleaseNote(version: "3.6.11", title: "App 里显示 Antigravity 的额度", points: [
            .init(symbol: "gauge.with.dots.needle.33percent",
                  text: "Antigravity 账号的账号页按组显示额度（Gemini 模型和 Claude、GPT 模型），每组照 Claude 的样子画 5 小时、本周；侧栏、菜单栏和总览卡片写各组里用得最多的那个。"),
            .init(symbol: "square.stack.3d.up",
                  text: "一组用完了，别的组的模型照样能用：所有组都用完才算额度用完。每查一次要十几秒，App 查得比 Claude、Codex 慢一些。"),
            .init(symbol: "sidebar.left",
                  text: "侧栏里套餐名太长时截断（比如 Google AI Pro），不再把用量的百分比挤出侧栏。"),
        ]),
        ReleaseNote(version: "3.6.10", title: "Antigravity 更新到 1.5.16 以后照样能用，还能查额度", points: [
            .init(symbol: "key",
                  text: "agy 更新到 1.5.16 以后 cca agy 会停下：新版会让各账号共用钥匙串里的同一份登录。现在 cca 让它照旧把登录存在各账号自己的目录里，以前的登录接着用。"),
            .init(symbol: "gauge.with.dots.needle.33percent",
                  text: "新增 cca agy usage：查各个 Antigravity 账号的额度，Gemini 模型和 Claude、GPT 模型各有本周、5 小时两种；cca agy ls 里也列出来。App 里暂时还不显示。"),
            .init(symbol: "person.badge.key",
                  text: "查额度时登录已经失效会直接告诉你，用 cca agy login 重新登录；这次查不到时显示上次查到的，并注明原因。"),
        ]),
        ReleaseNote(version: "3.6.9", title: "Claude App 里的会话也认得出账号", points: [
            .init(symbol: "person.crop.circle.badge.checkmark",
                  text: "这台设备有认不出账号的 Claude App 会话时，「折合金额」页合计下面有「记下账号…」（或者 cca cost hook on）：问过你以后在 ~/.claude/settings.json 里加一条 hook，App 里的 Code 会话开始时记下 App 登录的账号，以后的用量算进这个账号。"),
            .init(symbol: "arrow.triangle.swap",
                  text: "在别的账号下续接的会话，续接以后的用量算进新的账号（Claude Code 在会话记录里照旧写原来的账号）。"),
            .init(symbol: "arrow.triangle.2.circlepath",
                  text: "更新后重新统计一遍。不想记了用 cca cost hook off；关掉折合金额、卸载 cca 时这条 hook 自动去掉。"),
        ]),
        ReleaseNote(version: "3.6.8", title: "折合金额按模型分开，认不出账号的用量也看得到", points: [
            .init(symbol: "chart.bar.xaxis",
                  text: "「折合金额」页的合计和账号页的「折合金额」下面按模型列出金额和请求数（近 12 周，按日时近 30 天），Team 里各台设备的加在一起。"),
            .init(symbol: "questionmark.circle",
                  text: "认不出是哪个账号的用量（Claude App 里的 Code 会话、旧版 Claude Code）每台设备也同步到 Team，在「折合金额」页最下面单列一组，不算进合计。"),
            .init(symbol: "arrow.triangle.2.circlepath",
                  text: "补上 GPT-6.2 Astra、Sol 的价格。更新后重新统计一遍；别的设备也更新到 3.6.8 以后，才看得到它们按模型的和认不出账号的用量。"),
        ]),
        ReleaseNote(version: "3.6.7", title: "账号页的设备菜单只列用过这个账号的设备", points: [
            .init(symbol: "desktopcomputer",
                  text: "账号页「折合金额」右上角的设备菜单只列用过这个账号的设备，只在别的 Team 里的设备不再出现在这个账号下。"),
            .init(symbol: "chart.bar.xaxis",
                  text: "选着的设备没用过这个账号时，这一页看所有设备；「折合金额」页和那台设备用过的账号照样看它。"),
        ]),
        ReleaseNote(version: "3.6.6", title: "Team 里每台设备都看得到各台设备的折合金额", points: [
            .init(symbol: "chart.bar.xaxis",
                  text: "Team 给的 Claude、Codex 账号，这台设备没登录也看得到 Team 里各台设备的折合金额，账号页和「折合金额」页的设备菜单里能切到各台设备。"),
            .init(symbol: "trash",
                  text: "从 Team 里移除的、自己退出的设备不再列出：「共享与同步」和折合金额里都没有它；管理员「更新服务器」一次，服务器也删掉它同步过的折合金额。"),
            .init(symbol: "arrow.triangle.2.circlepath",
                  text: "Team 里的设备都更新到 3.6.6 以后才看得到彼此的；还没更新的设备传上来的，仍然要登录着那个账号才看得到。"),
        ]),
        ReleaseNote(version: "3.6.5", title: "App 里更新 Team 的服务器时照部署时的私钥登录", points: [
            .init(symbol: "key",
                  text: "App 里的「更新服务器」预填部署时记下的那台；填的还是它时照部署时用 -i 写的私钥登录，更新后接着记着，删除 Team 时也照它。"),
            .init(symbol: "terminal",
                  text: "终端里再部署同一台时，没写的 -p、-i、--dir 照上次的；装在别的目录的，更新时不会再在默认目录另装一份。"),
            .init(symbol: "arrow.triangle.2.circlepath",
                  text: "3.6.4 在 App 里更新过的，私钥没记下了：在终端里带着 -i 再部署一次，或者在 ~/.ssh/config 里给这台服务器写上 IdentityFile。"),
        ]),
        ReleaseNote(version: "3.6.4", title: "删除 Team 时用部署时写的私钥登录", points: [
            .init(symbol: "key",
                  text: "cca team deploy 时用 -i 写的私钥会记下来：删掉这个 Team（「删掉这个 Team…」或 cca team leave）时照它登录服务器、卸掉同步服务器，在 App 里删也行。"),
            .init(symbol: "lock",
                  text: "服务器不认这把私钥、也不让用密码时，不再让你把这台 Mac 的公钥加到服务器上，直接问要不要只删这台设备上的，并说是哪把私钥。"),
            .init(symbol: "arrow.triangle.2.circlepath",
                  text: "以前部署的没记下私钥：带着 -i 再部署一次（cca team deploy Team 用户@服务器 -i 私钥），或者在 ~/.ssh/config 里给这台服务器写上 IdentityFile。"),
        ]),
        ReleaseNote(version: "3.6.3", title: "删除 Team 时一起卸掉它的同步服务器", points: [
            .init(symbol: "trash",
                  text: "管理员删掉一个 Team（「删掉这个 Team…」或 cca team leave）时，先经 SSH 卸掉它的同步服务器：停掉服务，程序、证书和数据一起删掉，服务器上不再留下没人用的服务。"),
            .init(symbol: "key",
                  text: "这台 Mac 的密钥登录不了时问密码；卸不掉时问要不要仍然删掉、服务器留着。只想删这台设备上的：cca team leave Team --keep-server。"),
            .init(symbol: "server.rack",
                  text: "以前部署的照部署时的默认位置卸载（root@服务器、/opt/cca-team-编号的前 8 位），不对的话写清楚：cca team leave Team 用户@服务器 [-p 端口] [-i 私钥] [--dir 目录]。"),
        ]),
        ReleaseNote(version: "3.6.2", title: "侧栏的底栏不再挡住账号", points: [
            .init(symbol: "sidebar.left",
                  text: "侧栏要滚动时，滚到底栏下面的账号会淡出，不再和版本号、「新手教程」叠在一起。"),
            .init(symbol: "checkmark.shield",
                  text: "启动账号、cca check 时的 DNS 泄露检测偶尔会少发一个请求、甚至没测成：修好了。"),
        ]),
        ReleaseNote(version: "3.6.1", title: "Team 的管理员指定安装用的节点", points: [
            .init(symbol: "arrow.down.circle",
                  text: "Team 的管理员用 cca team node Team install 节点 指定：各台设备直连拿不到安装脚本时，经内置代理的这个节点装 claude、codex、agy 命令，不用在终端里现选。"),
            .init(symbol: "list.bullet",
                  text: "cca team node Team 列出节点时标着安装用的是哪个；写 install - 是不再指定。"),
        ]),
        ReleaseNote(version: "3.6.0", title: "去掉维护者的映射，只用 Team", points: [
            .init(symbol: "person.3",
                  text: "维护者共享的账号（cca policy）去掉了，账号和节点只经 Team 给：还没加入的设备向管理员要邀请链接，在「共享与同步」页或用 cca team join 加入。"),
            .init(symbol: "trash",
                  text: "更新后第一次运行时，清掉以前拉取的映射、deploy key 和经维护者的服务器同步的记录；账号表里的账号照旧能用。"),
            .init(symbol: "server.rack",
                  text: "远程会话只经 Team 的服务器中转：以前经维护者的服务器配的对不再显示，要在 Team 的服务器上重新配对。"),
        ]),
        ReleaseNote(version: "3.5.5", title: "新建 Team 时选上这台设备的账号", points: [
            .init(symbol: "checklist",
                  text: "新建 Team 时多一页：勾选这台设备上的哪些账号放进去，连同现在走的节点和别名。自己加的账号已经勾好，都不勾就先建一个空的 Team。"),
            .init(symbol: "person.3",
                  text: "这台设备当管理员的别的 Team 里的账号也能放进去，节点的定义一起带过来；第三方 API 账号和别人共享给你的不能放。"),
            .init(symbol: "terminal",
                  text: "终端里 cca team new 名字 也会列出来按编号选，或者直接写 --accounts（all、none，或者 claude:邮箱,codex:邮箱）。"),
        ]),
        ReleaseNote(version: "3.5.4", title: "更新后马上同步 Team 的配置", points: [
            .init(symbol: "arrow.triangle.2.circlepath",
                  text: "cca 更新到新版本以后，马上和所在的 Team 同步一次配置（平时每小时一次）。用着维护者的映射、还没加入 Team 的机器，更新以后马上自己加入，不用再等一天。"),
            .init(symbol: "list.number",
                  text: "「共享与同步」页 Team 的「账号的别名和节点」先按工具、再按别名排（team-2 在 team-10 前面），不再按邮箱排。"),
            .init(symbol: "character.bubble",
                  text: "「取配置」改叫「同步配置」，App 和命令行里的说法都跟着改了。"),
        ]),
        ReleaseNote(version: "3.5.3", title: "Team、在 iPhone 上接入远程会话", points: [
            .init(symbol: "person.3",
                  text: "「共享与同步」页多了 Team：一组设备共用一份配置（哪些账号、各走哪个节点）。管理员给每台设备发邀请链接，粘贴就加入；侧栏、总览和菜单栏面板能按 Team 筛选。"),
            .init(symbol: "arrow.triangle.2.circlepath",
                  text: "维护者共享的账号改由 Team 给：用着映射的机器拿到邀请后自己加入。「自己的同步服务器」去掉了，自己的设备之间也用 Team 同步（见 README 的「Team」一节）。"),
            .init(symbol: "server.rack",
                  text: "管理员一键部署 Team 的服务器（一台能装几个，带远程会话的中转、手机网页端和自动续期的证书）；能给设备、Team 改名，给账号起别名，用 cca team node 管节点。"),
            .init(symbol: "iphone",
                  text: "远程会话能在 iPhone 上接入：在「远程会话」页点「和手机配对…」，扫码后在 Safari 里输入配对码，再添加到主屏幕（iOS 17 以上）。"),
        ]),
        ReleaseNote(version: "3.5.2", title: "远程会话、自己的同步服务器、完全卸载", points: [
            .init(symbol: "terminal",
                  text: "远程会话：运行 cca remote on 以后，终端关了会话照样运行。侧栏的「远程会话」页能接入这台 Mac 和配对过的机器上的会话，可以同时接着几个。"),
            .init(symbol: "server.rack",
                  text: "「共享与同步」页多了「自己的同步服务器」：几台设备之间同步设备统计、折合金额和 API key，不经过维护者。维护者共享的账号改成默认关着，已经开着的照旧。"),
            .init(symbol: "checkmark.shield",
                  text: "cca 启动的 Claude Code 读写记忆、用共享的 skills 不再反复要确认。"),
            .init(symbol: "arrow.triangle.branch",
                  text: "本机 UDP 走节点：在家时自动停着，别的 VPN 接管流量时先让开。升级后在节点分流页的「本机 UDP」点一次「重新装…」。"),
            .init(symbol: "trash",
                  text: "终端里运行 cca uninstall 完全卸载，缓存和这个 App 的设置一起清掉。"),
        ]),
        ReleaseNote(version: "3.5.1", title: "账号可以改走你自己的代理", points: [
            .init(symbol: "link.badge.plus",
                  text: "维护者没固定节点的账号，在账号页「网络出口」点「自定义代理…」，粘贴 socks5://、http:// 地址（比如买的静态住宅 IP）或分享链接，就能改走这个代理。配置了路由器的会写进路由器的配置（要重启一次 OpenClash / ShellCrash 时先问你），在外面时内置代理照样用它。"),
            .init(symbol: "checkmark.shield",
                  text: "几台 Mac 共用一个路由器时，各自加的自定义代理互不覆盖：同名却不是同一个的，这台的自动改个名字（出口不变）；对不上的端口宁可断开，也不换成别的出口。"),
            .init(symbol: "dollarsign.circle",
                  text: "折合金额：以后经 cca 启动的旧版 Claude Code（2.1.316 及以前）的会话也算得进来。还有用量认不出是哪个账号的，折合金额页标出是哪台设备、为什么；命令行里的 Claude Code 太旧时提醒更新。"),
        ], art: "route"),
        ReleaseNote(version: "3.5.0", title: "cca 3.5 正式版：内嵌的 Python 换成了原生程序，更快、更省", points: [
            .init(symbol: "bolt",
                  text: "cca 里原来内嵌的 Python 换成了原生程序：一条命令里 cca 自己占的时间降到原来的几分之一（比如 cca ls 从约 0.5 秒到约 0.1 秒），后台的上报进程内存从约 31 MB 降到约 6 MB。用法、配置和文件都没变。"),
            .init(symbol: "shippingbox",
                  text: "不再需要 python3。cca 这个文件大了（约 13 MB，四种机器用的程序都接在里面），第一次运行时解出这台机器的那一份。"),
            .init(symbol: "arrow.uturn.backward",
                  text: "只支持 Intel / Apple 芯片的 Mac，x86_64 / arm64 的 Linux。升级以后发现和以前不一样的地方，可以用 cca update --rollback 退回上一个版本。"),
        ]),
        ReleaseNote(version: "3.4.1", title: "不再用内置代理时，开机自动启动它的那一行一起删掉", points: [
            .init(symbol: "trash",
                  text: "macOS 26 以下由 cca 在后台运行内置代理时，cca local off、cca restore 也会删掉 crontab 里开机自动启动内核的那一行（以前只在 Linux 上删）。"),
            .init(symbol: "text.bubble",
                  text: "用不了内置代理时，添加自定义节点的提示照实说是内核没有这台机器的架构，不再说要 macOS 26。"),
        ]),
        ReleaseNote(version: "3.4.0",title: "cca 3.4 正式版：配置向导三步走，这台 Mac 的 UDP 也能经节点出去", points: [
            .init(symbol: "list.number",
                  text: "配置向导三步走：连上维护者的映射、选这台 Mac 怎么联网、添加账号，配好的步骤自动跳过；添加账号不再要求先配置路由器。"),
            .init(symbol: "antenna.radiowaves.left.and.right",
                  text: "用内置代理时，这台 Mac 自己的 UDP（WebRTC、QUIC）也能经节点出去：节点分流页的「本机 UDP」，或者 cca local udp on。"),
            .init(symbol: "wrench.and.screwdriver",
                  text: "共享的 ~/.claude/settings.json 里配了第三方 API、代理时说清楚是哪几项；cca settings fix 先整份备份，再只去掉这几项。"),
            .init(symbol: "chart.bar",
                  text: "后台刷新用量时，侧栏里 Fable 那一行不再一闪就没了。"),
            .init(symbol: "desktopcomputer",
                  text: "总览里 App 开着、终端里有会话的账号一律算在用，有问题写成「在用 · 端口连不上」。"),
        ], art: "onboard"),
        ReleaseNote(version: "3.3.0", title: "cca 3.3 正式版：配置更简单，出门在外统一用内置代理", points: [
            .init(symbol: "sparkles",
                  text: "macOS 上一键安装后自动打开图形界面 App，第一次打开手把手引导你选这台 Mac 怎么联网、添加第一个账号，新手教程还配了图文步骤卡。"),
            .init(symbol: "bolt.horizontal.circle",
                  text: "出门在外连不上路由器时统一用 cca 自带的内置代理（全版本都能用），不用再装 Clash Verge；没有路由器时还能 cca node add 粘条分享链接自己加节点。"),
            .init(symbol: "envelope.badge",
                  text: "邮箱助手：登录邮件一到就自动在账号专属的浏览器里打开里面的链接（在账号页配置，只在 macOS 上）。"),
            .init(symbol: "network",
                  text: "两台 Mac 共用一个路由器不再互相覆盖端口；经 ZeroTier / VPN 连路由器不算在家，别的程序开着 TUN 也能用内置代理。"),
            .init(symbol: "checkmark.shield",
                  text: "节点分流页新增「检测全部出口」：一次把所有账号的出口和 DNS / WebRTC 泄露都测一遍，不用一个个点。"),
        ], art: "route"),
        ReleaseNote(version: "3.2.2", title: "换账号接着做同一个会话，任务清单也接得上", points: [
            .init(symbol: "checklist",
                  text: "Claude Code 在会话里列的任务清单以前每个账号各存一份，换账号恢复同一个会话后清单是空的。现在各账号共用一份；以前各存的会自动合到一起，同一个会话两边不一样时留较新的，另一份存进 ~/.cca/conflicts。"),
            .init(symbol: "arrow.triangle.2.circlepath",
                  text: "换账号接着做：先退出原来的 Claude Code（退出时会显示会话 ID），再在同一个项目目录里运行 cca @另一个账号 --resume 会话ID。工作流（Workflow）只能在原来的会话里续跑。"),
        ], art: "fan"),
        ReleaseNote(version: "3.2.1",title: "侧栏的工具切换、更新说明的按钮不再被挤掉", points: [
            .init(symbol: "sidebar.left",
                  text: "Claude、ChatGPT、Antigravity 三种工具都在用时，侧栏拖窄后账号列表上方的切换不再被截掉一截：放不下就换小一号，还放不下换成下拉菜单。只用两种工具时样子不变。"),
            .init(symbol: "book",
                  text: "「帮助」菜单的「更新说明」已经有三十多张，底下一排小圆点会把旁边的按钮挤得看不见字。超过 12 张时改成只写第几张、一共几张（比如「3 / 37」），翻页用旁边的按钮。"),
        ], art: "sidebar"),
        ReleaseNote(version: "3.2.0", title: "cca 3.2 正式版：看看每个账号的用量折合多少美元", points: [
            .init(symbol: "dollarsign.circle",
                  text: "左边新增「折合金额」：按 Anthropic、OpenAI 官方 API 的价格，把每个账号用掉的 token 折成美元（不是实际花费），可以切「本机 | 所有设备」「周 | 日」，账号页里也有。只读 Claude Code、Codex 会话记录里的用量数字，不读对话内容。"),
            .init(symbol: "laptopcomputer.and.iphone",
                  text: "维护者映射里的账号在几台设备上登录时，各台设备的折合金额、记下的 API key 经维护者的设备统计服务器加密同步，服务器看不到内容；旧版的服务器要维护者更新一次。"),
            .init(symbol: "arrow.down.app",
                  text: "「客户端更新」页多了「允许自动更新」（默认关）：查到新版本就直接装，桌面 App 开着时等你退出它再装。"),
            .init(symbol: "sidebar.left",
                  text: "左边的「Claude | ChatGPT」切换挪到了账号列表正上方，只换下面列出的账号。"),
        ], art: "quota"),
        ReleaseNote(version: "3.1.0", title: "cca 3.1 正式版", points: [
            .init(symbol: "checkmark.seal",
                  text: "这是 cca 3.1 正式版，包含 3.1.0 测试版到 beta.19 的全部内容：装在 Linux 服务器上、每周用量、账号在哪些设备上用、Antigravity 命令行等。装着测试版的机器会自动更新到它，以后照常只自动更新正式版。"),
        ], art: "logo"),
        ReleaseNote(version: "3.1.0-beta.19", title: "Antigravity 账号也有简写了", points: [
            .init(symbol: "terminal",
                  text: "aga 就是 cca agy，和 cxa 就是 cca codex 一样：aga @别名 直接启动这个 Antigravity 账号，aga add 添加账号。安装 cca 时和 cxa 一起建好；已经在用 Antigravity 的机器，升级后下一次运行 cca 时自动补上（已经有别的 aga 命令时不覆盖它）。"),
            .init(symbol: "checkmark.circle",
                  text: "登录 Antigravity 账号以后，主窗口上不再出现“后台刷新 Antigravity 的用量没有成功”：agy 本来就没有能在它外面查的用量接口，App 不再去查；等你登录的那段时间改成按时重读账号状态，登录好了界面照样会更新。"),
        ], art: "open"),
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
