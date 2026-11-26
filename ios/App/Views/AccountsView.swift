import CcaRemoteKit
import SwiftUI

/// 「账号」页：配对过的每台电脑一组，组里是它放上来的账号（照 Mac App 的总览：在用还是空闲、有什么问题、套餐、5 小时和本周的用量），
/// 点一个看它的详情。只能看，不在这里操作。账号的状态和会话列表是同一次查到的（AppModel 15 秒查一次，下拉也查）
struct AccountsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        TimelineView(.periodic(from: .now, by: 30)) { context in
            let now = context.date.timeIntervalSince1970
            let all = model.peers.compactMap { model.status($0) }.flatMap(\.accounts)
            List {
                if model.isDemo {
                    Section {
                        Banner(systemImage: "sparkles", tint: .accentColor, title: "演示模式",
                               message: "这里的电脑和账号是模拟的，不联网。") {
                            Button("退出演示") { model.leaveDemo() }.buttonStyle(.bordered)
                        }
                        .listRowInsets(EdgeInsets())
                        .listRowBackground(Color.clear)
                    }
                }
                if !all.isEmpty {
                    Section {
                        Text(Texts.census(AccountCensus(all, now: now)))
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                            .listRowInsets(EdgeInsets(top: 0, leading: 4, bottom: 0, trailing: 4))
                            .listRowBackground(Color.clear)
                    }
                    .listSectionSpacing(.compact)
                }
                ForEach(model.peers) { p in
                    MachineAccounts(peer: p, listing: model.listing(p), now: now)
                }
                Section {
                    Text("电脑上的 cca 每次查完用量把各个账号的状态封好放到服务器上，只有这台手机解得开（不给邮箱）。额度用完了、恢复了会推通知（设置里可以关）。")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .listRowBackground(Color.clear)
                }
            }
            .listStyle(.insetGrouped)
        }
        .navigationTitle("账号")
        .refreshable { await model.fetch() }
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                RemoteMark(size: 22).accessibilityHidden(true)
            }
            ToolbarItemGroup(placement: .topBarTrailing) {
                Button { model.startPairing() } label: { Image(systemName: "qrcode.viewfinder") }
                    .accessibilityLabel("和另一台电脑配对")
                Button { model.showSettings = true } label: { Image(systemName: "gearshape") }
                    .accessibilityLabel("设置")
            }
        }
    }
}

/// 一台电脑：标题行和会话那一页的一样（电脑图标、名字、Team），下面是它放上来的账号；下面一行说是多久以前放上来的
private struct MachineAccounts: View {
    @Environment(AppModel.self) private var model
    let peer: Peer
    let listing: PeerListing?
    let now: Double

    /// 放上来超过这么久没更新：那台电脑多半关机了、睡着了（没变的话它半小时放一次）
    static let oldAfter: Double = 2 * 3600

    var body: some View {
        Section {
            if let l = listing {
                if l.gone {
                    Label("那台电脑取消了和这台手机的配对。要用得重新配对。", systemImage: "info.circle")
                        .font(.callout).foregroundStyle(.secondary)
                    Button("从这里删掉", role: .destructive) { model.unpair(peer) }
                } else {
                    if let f = l.failure {
                        Label(Texts.listFailure(f.code), systemImage: "exclamationmark.triangle")
                            .font(.callout).foregroundStyle(.orange)
                    }
                    content(l)
                }
            } else {
                HStack { ProgressView(); Text("正在查…").foregroundStyle(.secondary) }.font(.callout)
            }
        } header: {
            HStack(spacing: 8) {
                Image(systemName: "desktopcomputer").foregroundStyle(.secondary).accessibilityHidden(true)
                Text(peer.name).font(.subheadline.weight(.semibold)).foregroundStyle(.primary).textCase(nil).lineLimit(1)
                if let team = peer.team { Tag(text: team) }
            }
        } footer: {
            if let st = listing?.status, !st.off {
                let age = max(now - st.putAt, 0)
                Text(Texts.agoPhrase(age) + "更新" + (age > Self.oldAfter ? "：那台电脑可能关机了、睡着了，醒来查过用量以后再更新" : ""))
                    .monospacedDigit()
            }
        }
    }

    @ViewBuilder
    private func content(_ l: PeerListing) -> some View {
        if let st = l.status {
            if st.off {
                Label("那台电脑现在不让这台手机接入，账号也不给看了。", systemImage: "hand.raised")
                    .font(.callout).foregroundStyle(.secondary)
            } else if st.accounts.isEmpty {
                Text("那台电脑上没有给这台手机看的账号（只给经这个配对的服务器的账号）。")
                    .font(.callout).foregroundStyle(.secondary)
            } else {
                ForEach(st.accounts) { a in
                    NavigationLink(value: AccountRef(pid: peer.pid, id: a.id)) {
                        AccountRow(account: a, now: now)
                    }
                }
                if st.more > 0 {
                    Text("还有 \(st.more) 个账号没放上来（一台电脑最多 \(HostStatus.maxAccounts) 个）")
                        .font(.footnote).foregroundStyle(.secondary)
                }
            }
        } else if l.failure == nil {
            Text(l.statusServed
                ? "那台电脑还没把账号的状态放上来：它上面的 cca 更新到最新版、查过一次用量以后就有了。"
                : "这台电脑所在的服务器还不支持在手机上看账号：要管理员更新服务器。")
                .font(.callout).foregroundStyle(.secondary)
        }
    }
}

/// 列表里的一个账号（照 Mac App 侧栏、总览里的那一行）：头像（在用时有绿点）、名字和套餐，下面是哪个工具、现在怎么样；右边是 5 小时、
/// 本周的小条
struct AccountRow: View {
    let account: AccountStatus
    let now: Double

    var body: some View {
        let a = account
        let m = AccountMood(a, now: now)
        HStack(spacing: 12) {
            AccountAvatar(account: a, size: 36)
            VStack(alignment: .leading, spacing: 3) {
                // 放不下套餐的徽章时只写名字（账号的那一页有）
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 6) {
                        name(a)
                        if !a.plan.isEmpty { PlanBadge(plan: a.plan, tier: a.tier) }
                    }
                    name(a)
                }
                MoodLine(mood: m, tool: a.tool, now: now)
            }
            Spacer(minLength: 6)
            if let u = a.usage, !MiniUsage.rows(u, now: now).isEmpty {
                MiniUsage(usage: u, now: now).layoutPriority(1)
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }

    private func name(_ a: AccountStatus) -> some View {
        Text(Texts.nonEmpty(a.name) ?? "账号").font(.body.weight(.medium)).lineLimit(1)
    }
}

/// 「Claude Code · 额度用完了，16:00 恢复」：工具是灰色的，状态照轻重上色（要动手的红色、用完了和快用完了橙色、建议先用绿色）
struct MoodLine: View {
    let mood: AccountMood
    let tool: String
    let now: Double

    var body: some View {
        Text("\(Text(UsageText.product(tool)).foregroundStyle(.secondary))\(Text(" · ").foregroundStyle(.tertiary))\(Text(Self.text(mood, now: now)).foregroundStyle(Self.color(mood)))")
            .font(.caption)
            .lineLimit(2)
    }

    static func text(_ m: AccountMood, now: Double) -> String {
        var s = Texts.mood(m, now: now)
        if m.nearLimit { s += " · 快用完了" }
        if m.useFirst != nil { s += " · 建议先用" }
        return s
    }

    static func color(_ m: AccountMood) -> Color {
        if let t = m.trouble { return t.needsAttention ? .red : .orange }
        if m.nearLimit { return .orange }
        if m.useFirst != nil { return .green }
        return .secondary
    }
}
