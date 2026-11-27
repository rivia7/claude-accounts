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

/// 头像、名字、工具和套餐、哪台电脑；下面一行是现在怎么样（有问题时说要做什么），建议先用时再一行
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
                    HStack(spacing: 6) {
                        Tag(text: UsageText.product(a.tool))
                        if !a.plan.isEmpty { PlanBadge(plan: a.plan, tier: a.tier) }
                    }
                    Label(peer.name, systemImage: "desktopcomputer")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
            .accessibilityElement(children: .combine)
            Label {
                Text(MoodLine.text(m, now: now) + Self.working(m, a, now: now) + Self.hint(m, a))
                    .fixedSize(horizontal: false, vertical: true)
            } icon: {
                Image(systemName: Self.icon(m))
            }
            .font(.callout)
            .foregroundStyle(MoodLine.color(m) == .secondary ? Color.primary : MoodLine.color(m))
            if let f = m.useFirst {
                Label(Texts.useFirst(f, now: now), systemImage: "arrow.up.forward.circle")
                    .font(.callout)
                    .foregroundStyle(.green)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    static func icon(_ m: AccountMood) -> String {
        switch m.trouble {
        case .usedUp?: return "hourglass"
        case .loggedOut?, .wrongAccount?, .expired?: return "exclamationmark.circle.fill"
        case nil: return m.busy ? "bolt.circle.fill" : m.nearLimit ? "gauge.with.dots.needle.67percent" : "checkmark.circle"
        }
    }

    /// 在用时电脑上开着什么：「 · Claude App 开着 · 终端里 2 个会话，已 2 小时」（旧版的宿主不说时不写）
    static func working(_ m: AccountMood, _ a: AccountStatus, now: Double) -> String {
        m.busy && (a.appOpen || a.cliSessions > 0) ? " · " + Texts.working(a, now: now) : ""
    }

    /// 要人动手的：在哪做（手机上做不了）
    static func hint(_ m: AccountMood, _ a: AccountStatus) -> String {
        switch m.trouble {
        case .loggedOut?, .expired?: return "。请在电脑上登录（例如运行 \(Texts.launchCommand(a))）"
        case .wrongAccount?: return "。请在电脑上退出，再用这个账号重新登录"
        default: return ""
        }
    }
}

/// 这个账号在那台电脑上开着的会话（照会话说明里的账号摘要认）：点一个接入，终端推到这一页上面，返回时回到这里
private struct AccountSessions: View {
    @Environment(AppModel.self) private var model
    let peer: Peer
    let account: AccountStatus
    let now: Double

    var body: some View {
        let sessions = model.listing(peer)?.sessions(of: account) ?? []
        VStack(alignment: .leading, spacing: 12) {
            Text("会话").font(.headline)
            if sessions.isEmpty {
                Text("电脑上暂无这个账号可接入的会话。在电脑的终端里运行 \(Texts.launchCommand(account)) 启动一个，就会显示在这里。")
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
        }
    }
}
