import CcaRemoteKit
import SwiftUI

/// 一个账号的那一页（照 Mac App 的账号页，只能看）：头像、名字、套餐、在哪台电脑、现在怎么样；用量（5 小时、本周、分模型的，
/// Antigravity 的几组）；每周用量的图；折合金额；这个账号在那台电脑上的会话，点一个接入。从折合金额页点进来的（focus 是 cost）
/// 打开时滚到「折合金额」那一节
struct AccountDetailView: View {
    enum Focus { case cost }

    @Environment(AppModel.self) private var model
    let ref: AccountRef
    var focus: Focus?
    /// 滚过了（只在头一次打开时滚：从终端返回时不再滚）
    @State private var focused = false

    var body: some View {
        TimelineView(.periodic(from: .now, by: 30)) { context in
            let now = context.date.timeIntervalSince1970
            if let found = model.account(ref) {
                let host = model.status(found.peer)
                ScrollViewReader { proxy in
                    ScrollView {
                        VStack(alignment: .leading, spacing: 26) {
                            TipCard(id: .account, message: Self.tip(found.account))
                            AccountHeader(peer: found.peer, account: found.account, now: now)
                            UsageSection(account: found.account, now: now)
                            // Antigravity 没有每周用量的统计（cca 按 5 小时、本周的窗口记，agy 没有这两个窗口：和 Mac App 一样）
                            if !found.account.isAPI, found.account.tool != "agy" {
                                WeeklySection(weeks: found.account.weeks, leftOut: host?.weeksLeftOut ?? false, now: now)
                            }
                            if CostSection.shows(found.account, host: host) {
                                CostSection(account: found.account, host: host, now: now).id(Focus.cost)
                            }
                            // 出口检测（Mac App 账号页的「网络出口」；旧版的 cca 不放上来，没有这一节）
                            if let net = found.account.net {
                                NetSection(account: found.account, net: net, now: now)
                            }
                            AccountSessions(peer: found.peer, account: found.account, now: now)
                        }
                        .padding(.horizontal, 20)
                        .padding(.top, 8)
                        .padding(.bottom, 30)
                        .pageColumn()
                        // 每周用量的图右边留出和下面折合金额的纵轴一样宽的一栏：同一周的柱子上下对齐（和 Mac App 一样）
                        .environment(\.weeklyGutter, CostChart.gutterLabels(found.account.cost, days: model.costDays))
                    }
                    .task {
                        guard focus == .cost, !focused, CostSection.shows(found.account, host: host) else { return }
                        focused = true
                        try? await Task.sleep(for: .milliseconds(250))
                        withAnimation { proxy.scrollTo(Focus.cost, anchor: .top) }
                    }
                }
                .navigationTitle(Texts.nonEmpty(found.account.name) ?? "账号")
            } else if let p = model.peers.first(where: { $0.pid == ref.pid }), model.listing(p) == nil {
                ProgressView("正在查询…")
            } else {
                ContentUnavailableView("找不到这个账号", systemImage: "person.crop.circle.badge.questionmark",
                                       description: Text("电脑上已没有这个账号，或已取消与这台手机的配对。"))
            }
        }
        .navigationBarTitleDisplayMode(.inline)
        .refreshable { await model.fetch() }
    }

    /// 顶上的提示卡片（Mac App 账号页的「这一页怎么看」）：手机上只能看，登录这类要在电脑上做
    static func tip(_ a: AccountStatus) -> String {
        let net = a.net != nil
        return "名字下方的提示说明这个账号现在能否使用，以及下一步该做什么（登录、检测出口这类操作要在电脑上做）。"
            + (a.isAPI ? (net ? "下方是接口的连接检测；" : "") + "最下面是这个账号在电脑上的会话，点击一个接入。"
               : "下方是用量、折合金额（点图上的一根柱子，下面那一行说那一项的细节）" + (net ? "和网络出口" : "")
                 + "；最下面是这个账号在电脑上的会话，点击一个接入。")
    }
}

/// 头像、名字、标签（工具、套餐，登录有问题时是登录状态：和 Mac App 账号页名字下面那一行一样）、哪台电脑；在用时下面一行说在用什么；
/// 再下面是「下一步」那句话（Advice）
private struct AccountHeader: View {
    let peer: Peer
    let account: AccountStatus
    let now: Double

    var body: some View {
        let a = account
        let m = AccountMood(a, now: now)
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 14) {
                AccountAvatar(account: a, size: 58)
                VStack(alignment: .leading, spacing: 6) {
                    Text(Texts.nonEmpty(a.name) ?? "账号").font(.title2.weight(.semibold)).lineLimit(1)
                    HStack(spacing: 6) { tags(a, m) }
                    // 哪台电脑；Team 给的账号后面是哪个 Team（总览按 Team 筛照它）
                    HStack(spacing: 10) {
                        Label(peer.name, systemImage: "desktopcomputer")
                        if let team = a.team { Label(team, systemImage: "person.3") }
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                }
            }
            .accessibilityElement(children: .combine)
            if m.busy {
                // 绿只表示在用（和 Mac App 的「在用」标签一样）
                Label(Texts.working(a, now: now), systemImage: "bolt.circle.fill")
                    .font(.callout)
                    .foregroundStyle(.green)
                    .fixedSize(horizontal: false, vertical: true)
            }
            AdviceLine(advice: Advice.make(a, mood: m, now: now))
        }
    }

    /// 哪个工具（Claude、ChatGPT、Antigravity）、套餐（第三方 API 账号写「第三方 API」和它的模型）；登录有问题时说登录状态
    @ViewBuilder
    private func tags(_ a: AccountStatus, _ m: AccountMood) -> some View {
        Tag(text: Texts.toolName(a.tool))
        if a.isAPI {
            Tag(text: "第三方 API", color: .blue, systemImage: "key.horizontal")
            if let model = a.model { Tag(text: model) }
        } else if !a.plan.isEmpty {
            PlanBadge(plan: a.plan, tier: a.tier)
        }
        switch m.trouble {
        case .loggedOut?: Tag(text: "未登录", color: .gray)
        case .wrongAccount?: Tag(text: "登录的不是这个账号", color: .orange)
        case .expired?: Tag(text: Texts.usageError("auth"), color: .orange)
        default: EmptyView()
        }
    }
}

/// 账号页顶上那句话（Mac App 的 Advice）：这个账号现在能不能用、下一步做什么。只照 AccountMood 说，不重算规则；先后照 Mac App 的
/// Diagnosis.all（取最要紧的那一条）：登录的问题、建议先用（只在没有挡路的问题时有）、出口有问题、额度用完（哪个窗口、几点恢复）、
/// 快用完（哪个窗口、用了多少、几点重置）、套餐过时、没装桌面 App、纯净度偏低、可能泄露、没检测过出口。登录、检测出口、同步路由器
/// 要在电脑上做（手机上做不了：写上在电脑上点哪个、运行哪条命令）；端口池的实况（端口连不上、节点选错了）排在出口前面，那台电脑的界面
/// App 开着时才有
struct Advice: Equatable {
    /// suggest：建议（建议先用），不是问题；info：只是提示（套餐过时、没装桌面 App、没检测过出口）；bad：用不了（出口有问题）
    enum Level { case ok, suggest, info, warn, bad }

    let level: Level
    let text: String

    static func make(_ a: AccountStatus, mood m: AccountMood, now: Double) -> Advice {
        switch m.trouble {
        case .loggedOut?:
            return Advice(level: .warn, text: "这个账号未登录：请在电脑上运行 \(Texts.loginCommand(a))，按提示登录。")
        case .wrongAccount?:
            return Advice(level: .warn, text: "登录的不是这个账号：请在电脑上运行 \(Texts.loginCommand(a))，换成这个账号重新登录。")
        case .expired?:
            return Advice(level: .warn, text: "登录已失效（比如改过密码）：请在电脑上运行 \(Texts.loginCommand(a))，按提示重新登录。")
        default:
            break
        }
        // 建议先用只在没有挡路的问题时有（AccountMood），有就排最前（Mac App 一样）
        if let f = m.useFirst { return Advice(level: .suggest, text: Texts.useFirst(f, now: now) + "。") }
        let check = "在电脑上点「\(a.isAPI ? "检测连接" : "检测出口")」（或运行 \(Texts.checkCommand(a))）"
        switch m.trouble {
        case .portClosed?:
            return Advice(level: .bad, text: "这个账号的端口无法连接：请在电脑上点「同步路由器」（或运行 cca router sync），仍不行请检查路由器。")
        case .wrongNode?:
            return Advice(level: .warn, text: "路由器给这个账号选错了节点：在电脑上点「同步路由器」（或运行 cca router sync）就能修好。")
        case .exit(let r, let region)?:
            return Advice(level: .bad, text: "出口有问题：\(Texts.healthResult(r, region: region))。\(check)重新检测。")
        case .usedUp?:
            let b = a.blocking(now: now)
            let name = b.map { UsageText.window($0.slot) + "的" } ?? ""
            let back = b?.resetsEpoch.map { "，" + UsageText.backAt(epoch: $0, now: now) } ?? ""
            return Advice(level: .warn, text: "\(name)额度已用完\(back)。可以先换个账号。")
        default:
            break
        }
        if m.nearLimit, let (slot, w, p) = Self.pressed(a, now: now) {
            let when = w.resetsEpoch.map { "，" + Texts.resetText(epoch: $0, now: now) } ?? ""
            return Advice(level: .warn, text: "\(UsageText.window(slot))的额度即将用完（\(Texts.percent(p))\(when)）。可以先换个账号。")
        }
        if let old = m.outdatedPlan {
            return Advice(level: .info, text: Texts.outdatedLoginPlan(old, current: a.plan) + "：在电脑上运行 \(Texts.loginCommand(a)) 重新登录后会更新。")
        }
        for f in m.flags {
            switch f {
            case .nearLimit:
                continue
            case .noApp:
                return Advice(level: .info, text: Texts.canWhy("app_missing", tool: a.tool) + "：可以先在电脑上用网页版。")
            case .lowPurity:
                return Advice(level: .warn, text: "出口 IP 的纯净度偏低，用的时候注意风险。")
            case .leak:
                return Advice(level: .warn, text: "DNS 或 WebRTC 可能泄露，详情看下面的网络出口。")
            case .neverChecked:
                return Advice(level: .info, text: a.isAPI ? "第一次使用这个接口？\(check)检查能否连接。"
                              : "第一次使用这个账号？先\(check)，查看出口位置和纯净度。")
            }
        }
        return Advice(level: .ok, text: "一切正常。")
    }

    /// 快用完了的那个窗口（90% 以上、还没重置，5 小时在前：和 AccountStatus.nearLimit 同一个规则）
    private static func pressed(_ a: AccountStatus, now: Double) -> (QuotaSlot, UsageWindow, Double)? {
        for (slot, w) in a.usage?.pressureWindows(now: now) ?? [] {
            if let w, let p = w.utilization, !w.isReset(now: now), p >= 90 { return (slot, w, p) }
        }
        return nil
    }
}

/// 「下一步」那句话：图标、颜色跟着轻重（和 Mac App 的 AdviceLine 一样：正常是绿的勾，建议先用是强调色的沙漏，提示是强调色的 i，
/// 要处理的是橙色的叹号，用不了的是红色的八角形）
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

/// 这个账号在那台电脑上开着的会话（照会话说明里的账号摘要认）：点一个接入，终端推到这一页上面，返回时回到这里。那台电脑让这台手机
/// 远程启动、有用得了这个账号的模板时，下面是「新建会话」（先选好这个账号，LaunchSheet）
private struct AccountSessions: View {
    @Environment(AppModel.self) private var model
    let peer: Peer
    let account: AccountStatus
    let now: Double

    var body: some View {
        let sessions = model.listing(peer)?.sessions(of: account) ?? []
        let launcher = model.launcher(peer)
        let templates = model.launchTemplates(on: peer, for: account)
        VStack(alignment: .leading, spacing: 12) {
            Text("会话").font(.headline)
            if sessions.isEmpty {
                Text(Self.empty(account, launch: launcher != nil && !templates.isEmpty, remoteOff: model.status(peer)?.remoteOn == false))
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(14)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
            } else {
                VStack(spacing: 0) {
                    ForEach(Array(sessions.enumerated()), id: \.element.id) { i, s in
                        if i > 0 { Divider().padding(.leading, 36) }
                        SessionRow(peer: peer, session: s, now: now)
                            .padding(.horizontal, 14)
                            .padding(.vertical, 11)
                    }
                }
                .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
            }
            if let launcher, !templates.isEmpty {
                Button {
                    model.launchFor = LaunchTarget(peer: peer, account: account.id)
                } label: {
                    Label("新建会话", systemImage: "plus.circle")
                }
                .buttonStyle(.glass)
                .disabled(!launcher.usable)
                .accessibilityHint(launcher.usable ? "用这个账号在电脑上启动一个新会话" : launcher.busy ? "电脑没有回应（可能已休眠）" : "电脑暂时没有连接服务器")
                if !launcher.usable {
                    Text("电脑暂时收不到启动请求（可能已休眠、断网，或远程启动的后台进程没有运行）。")
                        .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    /// 没有会话时那句话：怎么启动一个。那台电脑上没装这个工具的命令时终端里起不来；「远程会话」的开关关着时终端里启动的接不上
    /// （远程启动的照样接得上：它总是放进宿主里）
    static func empty(_ a: AccountStatus, launch: Bool, remoteOff: Bool) -> String {
        let none = "电脑上暂无这个账号可接入的会话。"
        let launchHint = launch ? "点「新建会话」在电脑上启动一个" : ""
        if a.whyCLI == "tool_missing" {
            return none + Texts.canWhy("tool_missing", tool: a.tool) + "，无法在终端里启动这个账号" + (launch ? "；可以" + launchHint : "") + "。"
        }
        if remoteOff {
            return none + (launch ? launchHint + "；" : "")
                + "电脑上「远程会话」的开关关着，在终端里启动的会话接不上：先在电脑上打开它（或运行 cca remote on）。"
        }
        return none + (launch ? launchHint + "，或在电脑的终端里运行 \(Texts.launchCommand(a))。"
                       : "在电脑的终端里运行 \(Texts.launchCommand(a)) 启动一个，就会显示在这里。")
    }
}

/// 网络出口（Mac App 账号页「网络出口」一节里出口检测的那一部分，HealthSummary）：最近一次检测的结果（标题旁边）、纯净度、DNS / WebRTC 的
/// 泄露检测、什么时候测的、过时了没有。线路（端口、节点、出口 IP、运营商）手机上没有：电脑不放上来（remote.md「手机上的账号」）；
/// 检测要在电脑上做（「检测出口」，或者运行 cca check 账号）
private struct NetSection: View {
    let account: AccountStatus
    let net: AccountNet
    let now: Double

    var body: some View {
        let a = account
        // 旧结果（过时、换了节点）是灰的：它说的不是现在的出口（和 Mac App 一样）
        let dim = net.stale || !net.applies
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("网络出口").font(.headline)
                Spacer(minLength: 6)
                if net.checked {
                    Tag(text: Texts.healthResult(net.result, region: net.region), color: dim ? .gray : Palette.health(net.result))
                }
            }
            VStack(alignment: .leading, spacing: 10) {
                if !net.checked {
                    Text(a.isAPI ? "尚未检测。在电脑上点「检测连接」（或运行 \(Texts.checkCommand(a))），检查接口能否连接。"
                         : "尚未检测。在电脑上点「检测出口」（或运行 \(Texts.checkCommand(a))），查看这个账号的出口 IP、地区、纯净度，以及 DNS / WebRTC 是否有泄露风险。")
                        .foregroundStyle(.secondary)
                } else {
                    if !net.applies {
                        Label("节点已更换：这是旧节点的结果，请在电脑上重新检测新的出口。", systemImage: "arrow.triangle.swap")
                    }
                    if !a.isAPI, net.trust != nil || net.dns != nil || net.webrtc != nil {
                        // 一行放不下时纯净度单独一行、DNS 和 WebRTC 一行，再放不下才一个一行
                        ViewThatFits(in: .horizontal) {
                            HStack(spacing: 6) { trustTag(dim); leakTags(dim) }
                            VStack(alignment: .leading, spacing: 6) { trustTag(dim); HStack(spacing: 6) { leakTags(dim) } }
                            VStack(alignment: .leading, spacing: 6) { trustTag(dim); leakTags(dim) }
                        }
                    }
                    HStack(spacing: 8) {
                        if let at = net.checkedEpoch {
                            Text(Texts.agoPhrase(now - at) + (net.atLaunch ? "启动时检测" : "检测"))
                        }
                        if net.stale {
                            Label("结果已经过时", systemImage: "clock.badge.exclamationmark").foregroundStyle(.orange)
                        }
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
            }
            .font(.callout)
            .fixedSize(horizontal: false, vertical: true)
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
        }
    }

    /// 纯净度的小标签（Mac App 的 HealthSummary：颜色跟着检测的结果）
    @ViewBuilder
    private func trustTag(_ dim: Bool) -> some View {
        if let t = net.trust {
            Tag(text: "纯净度 \(t)" + (net.trustLabel.map { " · \($0)" } ?? ""), color: dim ? .gray : Palette.health(net.result))
        }
    }

    /// 泄露检测的小标签（Mac App 的 HealthSummary：有泄露检测时 DNS、WebRTC 两个都放）
    @ViewBuilder
    private func leakTags(_ dim: Bool) -> some View {
        if net.dns != nil || net.webrtc != nil {
            Tag(text: Texts.leak(net.dns, what: "DNS"), color: dim ? .gray : Palette.leak(net.dns))
            Tag(text: Texts.leak(net.webrtc, what: "WebRTC"), color: dim ? .gray : Palette.leak(net.webrtc))
        }
    }
}
