import CcaRemoteKit
import SwiftUI

/// 「账号」页（照 Mac App 侧栏、菜单栏面板的样子）：上面切看哪个工具的账号（Claude、ChatGPT、Antigravity；只有一个工具有账号时不放），
/// 可以只看一档套餐（AccountShelf，选的记着）；下面配对过的每台电脑一组，组里是它放上来的这个工具（这一档）的账号（照 Mac App 的总览：
/// 在用还是空闲、有什么问题、套餐、5 小时和本周的用量），点一个看它的详情。只能看，不在这里操作。账号的状态和会话列表是同一次查到的
/// （AppModel 15 秒查一次，下拉也查）
struct AccountsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        TimelineView(.periodic(from: .now, by: 30)) { context in
            let now = context.date.timeIntervalSince1970
            let all = model.peers.compactMap { model.status($0) }.flatMap(\.accounts)
            let shelf = AccountShelf(all, tool: model.accountTool, tier: model.accountTier)
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
                        // 这一行的底色是透明的，可分组列表照样按行的圆角（iOS 26 的圆角大）裁内容：上下留出一点，左下角那一句的头一个字才不被裁掉
                        ShelfHeader(shelf: shelf, now: now)
                            .listRowInsets(EdgeInsets(top: 4, leading: 4, bottom: 12, trailing: 4))
                            .listRowBackground(Color.clear)
                    }
                    .listSectionSpacing(.compact)
                }
                ForEach(model.peers) { p in
                    MachineAccounts(peer: p, listing: model.listing(p), shelf: shelf, now: now)
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

/// 页头：工具切换（分段，几个工具有账号时才放）、按套餐档位筛（所有账号里有两档以上时才放），下面一句点名（列着的这些账号：几个，几个在用……）；
/// 筛着的那一档在这个工具里没有账号时换成一句话和「看全部套餐」
private struct ShelfHeader: View {
    @Environment(AppModel.self) private var model
    let shelf: AccountShelf
    let now: Double

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if shelf.tools.count > 1 {
                Picker("看哪个工具的账号", selection: Binding(get: { shelf.tool }, set: { model.accountTool = $0 })) {
                    ForEach(shelf.tools, id: \.self) { t in Text(Texts.toolName(t)).tag(t) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
            }
            if !shelf.tierChips.isEmpty {
                TierFilter(shelf: shelf) { model.accountTier = $0 }
            }
            if shelf.shown.isEmpty, !shelf.tier.isEmpty {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(Texts.tierEmpty(shelf.tier, tool: shelf.tool)).foregroundStyle(.secondary)
                    Button("看全部套餐") { model.accountTier = "" }
                        .buttonStyle(.borderless)
                }
                .font(.footnote)
            } else {
                Text(Texts.census(AccountCensus(shelf.shown, now: now), tool: shelf.tool))
                    .font(.footnote)
                    .foregroundStyle(.secondary)            }
        }
        .padding(.vertical, 2)    }
}

/// 按套餐档位筛（照 Mac App 侧栏的 Team 筛选，只管显示）：「全部」和这个工具的账号里有的那几档（入门 → 尊贵，AccountShelf.tierChips），
/// 一档一个胶囊：档位的小徽记（和套餐徽章上的一样）、叫法、这个工具这一档有几个。选着的那个底色深一些、带边；筛着的那一档在这个工具里没有账号时
/// 也列着（个数是 0）。放不下时先不写个数，再放不下左右划
private struct TierFilter: View {
    let shelf: AccountShelf
    let pick: (String) -> Void

    var body: some View {
        ViewThatFits(in: .horizontal) {
            chips(counts: true)
            chips(counts: false)
            ScrollView(.horizontal) { chips(counts: false) }
                .scrollIndicators(.hidden)
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("按套餐档位筛")
    }

    private func chips(counts: Bool) -> some View {
        HStack(spacing: 8) {
            chip("", counts: counts)
            ForEach(shelf.tierChips, id: \.self) { chip($0, counts: counts) }
        }
    }

    private func chip(_ t: String, counts: Bool) -> some View {
        let on = shelf.tier == t
        let n = shelf.count(t)
        let name = t.isEmpty ? "全部" : Texts.planTier(t) ?? t
        let shape = Capsule()
        return Button { pick(t) } label: {
            HStack(spacing: 5) {
                if !t.isEmpty { TierMark(tier: t, size: 10) }
                Text(name).foregroundStyle(on ? Color.primary : Color.secondary)
                if counts { Text("\(n)").monospacedDigit().foregroundStyle(.tertiary) }
            }
            .font(.footnote.weight(on ? .semibold : .medium))
            .lineLimit(1)
            .padding(.horizontal, 11)
            .frame(height: 30)
            .background(shape.fill(on ? Color.primary.opacity(0.12) : Surface.tile))
            .overlay(shape.strokeBorder(on ? Color.primary.opacity(0.28) : Surface.hairline, lineWidth: on ? 1 : 0.5))
            .contentShape(shape)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(t.isEmpty ? "全部套餐" : "\(name)套餐")
        .accessibilityValue("\(n) 个账号")
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

/// 一台电脑：标题行和会话那一页的一样（电脑图标、名字、Team），下面是它放上来的这个工具（这一档）的账号；下面一行说是多久以前放上来的。
/// 放上来的账号里一个也不是这个工具（这一档）的电脑不列；那台电脑本身有要说的（还在查、取消了配对、没问到、还没放、不让看）照样列
private struct MachineAccounts: View {
    @Environment(AppModel.self) private var model
    let peer: Peer
    let listing: PeerListing?
    let shelf: AccountShelf
    let now: Double

    /// 放上来超过这么久没更新：那台电脑多半关机了、睡着了（没变的话它半小时放一次）
    static let oldAfter: Double = 2 * 3600

    var body: some View {
        let rows = listing?.status.map { shelf.rows($0.accounts) } ?? []
        if !Self.skipped(listing, rows: rows) {
            section(rows)
        }
    }

    /// 这一组不列：那台电脑放上来了账号，可是没有这个工具（这一档）的
    static func skipped(_ l: PeerListing?, rows: [AccountStatus]) -> Bool {
        guard let l, !l.gone, l.failure == nil, let st = l.status, !st.off, !st.accounts.isEmpty else { return false }
        return rows.isEmpty
    }

    private func section(_ rows: [AccountStatus]) -> some View {
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
                    content(l, rows: rows)
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
    private func content(_ l: PeerListing, rows: [AccountStatus]) -> some View {
        if let st = l.status {
            if st.off {
                Label("那台电脑现在不让这台手机接入，账号也不给看了。", systemImage: "hand.raised")
                    .font(.callout).foregroundStyle(.secondary)
            } else if st.accounts.isEmpty {
                Text("那台电脑上没有给这台手机看的账号（只给经这个配对的服务器的账号）。")
                    .font(.callout).foregroundStyle(.secondary)
            } else {
                ForEach(rows) { a in
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

/// 列表里的一个账号（照 Mac App 侧栏、总览里的那一行）：头像（在用时有绿点）、名字和套餐，下面是现在怎么样（不写工具：上面切的就是它；
/// 第三方 API 账号写上）；右边是 5 小时、本周的小条
struct AccountRow: View {
    let account: AccountStatus
    let now: Double

    var body: some View {
        let a = account
        let m = AccountMood(a, now: now)
        HStack(spacing: 12) {
            AccountAvatar(account: a, size: 36)
            VStack(alignment: .leading, spacing: 3) {
                // 放不下套餐的徽章时只放档位的小徽记（按档位筛的时候看得出是哪一档），还放不下只写名字（账号的那一页有）
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 6) {
                        name(a)
                        if !a.plan.isEmpty { PlanBadge(plan: a.plan, tier: a.tier) }
                    }
                    if !a.plan.isEmpty, AccountShelf.knownTiers.contains(a.tier) {
                        HStack(spacing: 6) {
                            name(a)
                            TierMark(tier: a.tier, size: 10)
                                .accessibilityElement(children: .ignore)
                                .accessibilityLabel(Texts.planName(a.plan, tier: a.tier))
                        }
                    }
                    name(a)
                }
                MoodLine(mood: m, prefix: a.isAPI ? "第三方 API" : nil, now: now)
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

/// 「额度用完了，16:00 恢复」：状态照轻重上色（要动手的红色、用完了和快用完了橙色、建议先用绿色）；给了 prefix 的（第三方 API 账号）
/// 前面是灰色的「第三方 API · 」
struct MoodLine: View {
    let mood: AccountMood
    var prefix: String?
    let now: Double

    var body: some View {
        let state = Text(Self.text(mood, now: now)).foregroundStyle(Self.color(mood))
        Group {
            if let prefix {
                Text("\(Text(prefix).foregroundStyle(.secondary))\(Text(" · ").foregroundStyle(.tertiary))\(state)")
            } else {
                state
            }
        }
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
