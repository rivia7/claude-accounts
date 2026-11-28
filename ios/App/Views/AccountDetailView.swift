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
                            AccountHeader(peer: found.peer, account: found.account, now: now)
                            UsageSection(account: found.account, now: now)
                            // Antigravity 没有每周用量的统计（cca 按 5 小时、本周的窗口记，agy 没有这两个窗口：和 Mac App 一样）
                            if !found.account.isAPI, found.account.tool != "agy" {
                                WeeklySection(weeks: found.account.weeks, leftOut: host?.weeksLeftOut ?? false, now: now)
                            }
                            if CostSection.shows(found.account, host: host) {
                                CostSection(account: found.account, host: host, now: now).id(Focus.cost)
                            }
                            AccountSessions(peer: found.peer, account: found.account, now: now)
                        }
                        .padding(.horizontal, 20)
                        .padding(.top, 8)
                        .padding(.bottom, 30)
                        .pageColumn()
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
                    Label(peer.name, systemImage: "desktopcomputer")
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

    /// 哪个工具（Claude、ChatGPT、Antigravity）、套餐（第三方 API 账号写「第三方 API」）；登录有问题时说登录状态
    @ViewBuilder
    private func tags(_ a: AccountStatus, _ m: AccountMood) -> some View {
        Tag(text: Texts.toolName(a.tool))
        if a.isAPI {
            Tag(text: "第三方 API", color: .blue, systemImage: "key.horizontal")
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

/// 账号页顶上那句话（Mac App 的 Advice）：这个账号现在能不能用、下一步做什么。只照 AccountMood 说，不重算规则；最要紧的一条排最前：
/// 登录的问题（要在电脑上登录：手机上做不了，写上在电脑上运行哪条命令）、额度用完（哪个窗口、几点恢复）、快用完（哪个窗口、用了多少、
/// 几点重置）、建议先用。端口、出口这些 Mac 才知道的不说
struct Advice: Equatable {
    /// suggest：建议（建议先用），不是问题
    enum Level { case ok, suggest, warn }

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
        case .usedUp?:
            let b = a.blocking(now: now)
            let name = b.map { UsageText.window($0.slot) + "的" } ?? ""
            let back = b?.resetsEpoch.map { "，" + UsageText.backAt(epoch: $0, now: now) } ?? ""
            return Advice(level: .warn, text: "\(name)额度已用完\(back)。可以先换个账号。")
        case nil:
            break
        }
        if m.nearLimit, let (slot, w, p) = Self.pressed(a, now: now) {
            let when = w.resetsEpoch.map { "，" + Texts.resetText(epoch: $0, now: now) } ?? ""
            return Advice(level: .warn, text: "\(UsageText.window(slot))的额度即将用完（\(Texts.percent(p))\(when)）。可以先换个账号。")
        }
        if let f = m.useFirst { return Advice(level: .suggest, text: Texts.useFirst(f, now: now) + "。") }
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

/// 「下一步」那句话：图标、颜色跟着轻重（和 Mac App 的 AdviceLine 一样：正常是绿的勾，建议先用是强调色的沙漏，要处理的是橙色的叹号）
struct AdviceLine: View {
    let advice: Advice

    var body: some View {
        let (icon, color): (String, Color) = {
            switch advice.level {
            case .ok: return ("checkmark.circle.fill", .green)
            case .suggest: return ("hourglass.circle.fill", .accentColor)
            case .warn: return ("exclamationmark.circle.fill", .orange)
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
                Text(templates.isEmpty
                     ? "电脑上暂无这个账号可接入的会话。在电脑的终端里运行 \(Texts.launchCommand(account)) 启动一个，就会显示在这里。"
                     : "电脑上暂无这个账号可接入的会话。点「新建会话」在电脑上启动一个，或在电脑的终端里运行 \(Texts.launchCommand(account))。")
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
                .disabled(!launcher.online)
                .accessibilityHint(launcher.online ? "用这个账号在电脑上启动一个新会话" : "电脑暂时没有连接服务器")
                if !launcher.online {
                    Text("电脑暂时收不到启动请求（可能已休眠、断网，或远程启动的后台进程没有运行）。")
                        .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }
}
