import CcaRemoteKit
import SwiftUI

/// 「总览」（Mac App 的总览，首页）：上面切看哪个工具的账号（Claude、ChatGPT、Antigravity；只有一个工具有账号时不放），可以只看一种
/// 套餐（AccountShelf，选的记着），下面一句点名；配对过的每台电脑一组，一个账号一张卡片（插画是它此刻的状态：在打字是在用，趴在桌上是
/// 额度用完、显示器上写几点恢复，椅子空着是未登录，灰色的人是登录的不是这个账号），点一张看它的详情。只能看，不在这里操作。
/// 账号的状态和会话列表是同一次查到的（AppModel 15 秒查一次，下拉也查）
struct AccountsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        TimelineView(.periodic(from: .now, by: 30)) { context in
            let now = context.date.timeIntervalSince1970
            let all = model.peers.compactMap { model.status($0) }.flatMap(\.accounts)
            let shelf = AccountShelf(all, tool: model.accountTool, plan: model.accountPlan)
            let animate = !reduceMotion && !ProcessInfo.processInfo.isLowPowerModeEnabled
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    if model.isDemo {
                        Banner(systemImage: "sparkles", tint: .accentColor, title: "演示模式", message: "这里的电脑和账号是模拟的，不联网。") {
                            Button("退出演示") { model.leaveDemo() }.buttonStyle(.bordered)
                        }
                    }
                    if !all.isEmpty { ShelfHeader(shelf: shelf, now: now) }
                    ForEach(model.peers) { p in
                        MachineAccounts(peer: p, listing: model.listing(p), shelf: shelf, now: now, animate: animate)
                    }
                    Text("电脑上的 cca 每次查询用量后，会把账号状态加密上传，只有这台手机能解密（不含邮箱）。额度用完或恢复时会通知你（可在设置中关闭）。")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.horizontal, 20)
                .padding(.top, 8)
                .padding(.bottom, 30)
                .pageColumn()
            }
        }
        .navigationTitle("总览")
        .refreshable { await model.fetch() }
        .toolbar { PageToolbar(mark: { OverviewMark(size: 22) }) }
    }
}

/// 各页左上角的图标（和 Mac App 侧栏里这一页的图标一样）、右上角的配对和设置
struct PageToolbar<Mark: View, Extra: View>: ToolbarContent {
    @Environment(AppModel.self) private var model
    @ViewBuilder let mark: () -> Mark
    /// 这一页自己的按钮，放在配对、设置前面（会话列表的「排序」）；别的页没有
    @ViewBuilder let extra: () -> Extra

    var body: some ToolbarContent {
        ToolbarItem(placement: .topBarLeading) {
            mark().accessibilityHidden(true)
        }
        ToolbarItemGroup(placement: .topBarTrailing) {
            extra()
            Button { model.startPairing() } label: { Image(systemName: "qrcode.viewfinder") }
                .accessibilityLabel("和另一台电脑配对")
            Button { model.showSettings = true } label: { Image(systemName: "gearshape") }
                .accessibilityLabel("设置")
        }
    }
}

extension PageToolbar where Extra == EmptyView {
    init(@ViewBuilder mark: @escaping () -> Mark) {
        self.init(mark: mark, extra: { EmptyView() })
    }
}

/// 页头：工具切换（分段，几个工具有账号时才放）、按套餐筛（这个工具的账号里有两种以上套餐时才放），下面一句点名（列着的这些账号：几个，几个在用……）
private struct ShelfHeader: View {
    @Environment(AppModel.self) private var model
    let shelf: AccountShelf
    let now: Double

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if shelf.tools.count > 1 {
                Picker("选择工具", selection: Binding(get: { shelf.tool }, set: { model.accountTool = $0 })) {
                    ForEach(shelf.tools, id: \.self) { t in Text(Texts.toolName(t)).tag(t) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
            }
            if !shelf.planChips.isEmpty {
                PlanFilter(shelf: shelf) { model.accountPlan = $0 }
            }
            Text(Texts.census(AccountCensus(shelf.shown, now: now), tool: shelf.tool))
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
        .padding(.vertical, 2)    }
}

/// 按套餐筛（照 Mac App 侧栏的 Team 筛选，只管显示）：「全部」和这个工具的账号里有的那几种套餐（AccountShelf.planChips，档位从低到高），
/// 一种一个胶囊：档位的小徽记（和套餐徽章上的一样）、套餐的原名（Pro、Max、Plus……）、这个工具里有几个。选着的那个底色深一些、带边。
/// 放不下时先不写个数，再放不下左右划
private struct PlanFilter: View {
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
        .accessibilityLabel("按套餐筛选")
    }

    private func chips(counts: Bool) -> some View {
        HStack(spacing: 8) {
            chip(nil, counts: counts)
            ForEach(shelf.planChips, id: \.self) { chip($0, counts: counts) }
        }
    }

    private func chip(_ c: AccountShelf.PlanChip?, counts: Bool) -> some View {
        let p = c?.plan ?? ""
        let on = shelf.plan == p
        let n = shelf.count(p)
        let name = c?.plan ?? "全部"
        let shape = Capsule()
        return Button { pick(p) } label: {
            HStack(spacing: 5) {
                if let c, AccountShelf.knownTiers.contains(c.tier) { TierMark(tier: c.tier, size: 10) }
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
        .accessibilityLabel(c.map { Texts.planName($0.plan) } ?? "全部套餐")
        .accessibilityValue("\(n) 个账号")
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

/// 一台电脑：标题行和会话那一页的一样（电脑图标、名字、Team），下面是它放上来的这个工具（这个套餐）的账号，一个一张卡片；
/// 下面一行说是多久以前放上来的。放上来的账号里一个也不是这个工具（这个套餐）的电脑不列；那台电脑本身有要说的（还在查、取消了配对、
/// 没问到、还没放、不让看）照样列
private struct MachineAccounts: View {
    @Environment(AppModel.self) private var model
    let peer: Peer
    let listing: PeerListing?
    let shelf: AccountShelf
    let now: Double
    let animate: Bool

    /// 放上来超过这么久没更新：那台电脑多半关机了、睡着了（没变的话它半小时放一次）
    static let oldAfter: Double = 2 * 3600

    var body: some View {
        let rows = listing?.status.map { shelf.rows($0.accounts) } ?? []
        if !Self.skipped(listing, rows: rows) {
            VStack(alignment: .leading, spacing: 10) {
                MachineHeader(peer: peer)
                if let l = listing {
                    if l.gone {
                        note("对方已取消配对，需要重新配对才能继续使用。", systemImage: "info.circle")
                        Button("移除", role: .destructive) { model.unpair(peer) }.buttonStyle(.bordered)
                    } else {
                        if let f = l.failure {
                            note(Texts.listFailure(f.code), systemImage: "exclamationmark.triangle", tint: .orange)
                        }
                        content(l, rows: rows)
                    }
                } else {
                    HStack(spacing: 8) { ProgressView(); Text("正在查询…").foregroundStyle(.secondary) }.font(.callout)
                }
                if let st = listing?.status, !st.off {
                    let age = max(now - st.putAt, 0)
                    Text(Texts.agoPhrase(age) + "更新" + (age > Self.oldAfter ? "：电脑可能已关机或休眠，唤醒并查询用量后会再更新" : ""))
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    /// 这一组不列：那台电脑放上来了账号，可是没有这个工具（这个套餐）的
    static func skipped(_ l: PeerListing?, rows: [AccountStatus]) -> Bool {
        guard let l, !l.gone, l.failure == nil, let st = l.status, !st.off, !st.accounts.isEmpty else { return false }
        return rows.isEmpty
    }

    private func note(_ text: String, systemImage: String, tint: Color = .secondary) -> some View {
        Label(text, systemImage: systemImage)
            .font(.callout)
            .foregroundStyle(tint)
            .fixedSize(horizontal: false, vertical: true)
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Surface.tile))
    }

    @ViewBuilder
    private func content(_ l: PeerListing, rows: [AccountStatus]) -> some View {
        if let st = l.status {
            if st.off {
                note("电脑已不允许这台手机接入，账号也不再显示。", systemImage: "hand.raised")
            } else if st.accounts.isEmpty {
                note("电脑上没有可以显示的账号（只显示经同一台服务器中转的账号）。", systemImage: "person.crop.circle.badge.questionmark")
            } else {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 150, maximum: 300), spacing: 12)], alignment: .leading, spacing: 12) {
                    ForEach(rows) { a in
                        DeskCard(peer: peer, account: a, now: now, animate: animate)
                    }
                }
                if st.more > 0 {
                    Text("另有 \(st.more) 个账号未显示（每台电脑最多显示 \(HostStatus.maxAccounts) 个）")
                        .font(.footnote).foregroundStyle(.secondary)
                }
            }
        } else if l.failure == nil {
            note(l.statusServed
                 ? "电脑还没上传账号状态：把电脑上的 cca 更新到最新版并查询一次用量后就会显示。"
                 : "这台电脑使用的服务器还不支持在手机上查看账号，需要管理员更新服务器。", systemImage: "clock")
        }
    }
}

/// 一台电脑的标题行（总览、用量统计、折合金额、共享与同步几页共用）：电脑图标、名字、Team
struct MachineHeader: View {
    let peer: Peer

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "desktopcomputer").foregroundStyle(.secondary).accessibilityHidden(true)
            Text(peer.name).font(.subheadline.weight(.semibold)).lineLimit(1)
            if let team = peer.team { Tag(text: team) }
        }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isHeader)
    }
}

/// 一个账号的卡片（Mac App 总览的 DeskCard）：插画、名字和套餐、现在怎么样、5 小时和本周的小条。整张卡片点了进这个账号的那一页。
/// 插画的左上角是在用什么（桌面 App、命令行里几个会话），右上角是额度即将用完（Mac 那里还有出口、泄露的几样，手机上不知道），
/// 左下角是还能重置几次额度和最早什么时候过期，右下角是建议先用
private struct DeskCard: View {
    let peer: Peer
    let account: AccountStatus
    let now: Double
    let animate: Bool

    var body: some View {
        let a = account
        let m = AccountMood(a, now: now)
        let shape = RoundedRectangle(cornerRadius: 18, style: .continuous)
        NavigationLink(value: AccountRef(pid: peer.pid, id: a.id)) {
            VStack(alignment: .leading, spacing: 8) {
                DeskScene(account: a, look: DeskLook(a, mood: m), animate: animate, now: now)
                    .overlay(alignment: .topLeading) { workBadge(a, m) }
                    .overlay(alignment: .topTrailing) { if m.nearLimit { nearLimitBadge } }
                    .overlay(alignment: .bottomLeading) { resetBadge(a) }
                    .overlay(alignment: .bottomTrailing) { if m.useFirst != nil { useFirstBadge } }
                info(a, m)
            }
            .padding(10)
            .frame(maxHeight: .infinity, alignment: .top)
            .background(shape.fill(Surface.tile))
            .contentShape(shape)
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label(a, m))
        .accessibilityHint("打开这个账号的页面")
    }

    private func info(_ a: AccountStatus, _ m: AccountMood) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 6) {
                    name(a)
                    Spacer(minLength: 4)
                    if a.isAPI {
                        Text("第三方 API").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    } else if !a.plan.isEmpty {
                        PlanBadge(plan: a.plan, tier: a.tier)
                    }
                }
                HStack(spacing: 6) {
                    name(a)
                    Spacer(minLength: 4)
                    if !a.isAPI, AccountShelf.knownTiers.contains(a.tier) { TierMark(tier: a.tier, size: 10) }
                }
            }
            HStack(alignment: .firstTextBaseline, spacing: 5) {
                moodMark(m)
                // 灰字（和 Mac 一样：颜色只在前面的小记号上）；快用完了、建议先用是插画角上的标记
                Text(Texts.mood(m, now: now))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let u = a.usage, !MiniUsage.rows(u, now: now).isEmpty {
                MiniUsage(usage: u, now: now)
            }
        }
    }

    private func name(_ a: AccountStatus) -> some View {
        Text(Texts.nonEmpty(a.name) ?? "账号").font(.callout.weight(.semibold)).lineLimit(1).truncationMode(.middle)
    }

    /// 状态前面的小记号（和 Mac App 的一样）：颜色只表达状态（在用是绿点；在用、又有问题时是问题的记号）；账号不符、登录失效是橙点，
    /// 未登录是灰点，额度用完是灰色的月亮，空闲不画
    @ViewBuilder
    private func moodMark(_ m: AccountMood) -> some View {
        switch m.trouble {
        case .usedUp?: Image(systemName: "moon.zzz.fill").font(.caption2).foregroundStyle(.secondary)
        case .loggedOut?: Circle().fill(Color.gray).frame(width: 6, height: 6)
        case .wrongAccount?, .expired?: Circle().fill(Color.orange).frame(width: 6, height: 6)
        case nil: if m.busy { Circle().fill(Color.green).frame(width: 6, height: 6) }
        }
    }

    /// 右上角：额度即将用完（橙色，和 Mac App 右上角的标记一样）
    private var nearLimitBadge: some View {
        Image(systemName: "exclamationmark.circle.fill")
            .font(.callout)
            .foregroundStyle(.orange)
            .background(Circle().fill(.background).padding(1))
            .padding(4)
    }

    /// 左上角：在用什么（桌面 App、命令行里几个会话）
    @ViewBuilder
    private func workBadge(_ a: AccountStatus, _ m: AccountMood) -> some View {
        if m.busy, a.appOpen || a.cliSessions > 0 {
            HStack(spacing: 4) {
                if a.appOpen { Image(systemName: "macwindow") }
                if a.cliSessions > 0 {
                    Image(systemName: "terminal")
                    if a.cliSessions > 1 { Text("\(a.cliSessions)").monospacedDigit() }
                }
            }
            .sceneBadge()
        }
    }

    /// 左下角：还能重置几次额度，一张小票一次（太多了写 +N），旁边是最早什么时候过期（和 Mac App 一样）
    @ViewBuilder
    private func resetBadge(_ a: AccountStatus) -> some View {
        if let rc = a.usage?.resetCredits, let n = rc.availableCount, n > 0 {
            let drawn = min(n, 5)
            HStack(spacing: 3) {
                ForEach(0..<drawn, id: \.self) { _ in Image(systemName: "ticket.fill") }
                if n > drawn { Text("+\(n - drawn)").monospacedDigit() }
                if let epoch = rc.nextExpiresEpoch {
                    Text(Texts.expiryText(epoch: epoch, now: now)).lineLimit(1).padding(.leading, 2)
                }
            }
            .sceneBadge()
        }
    }

    /// 右下角：本周额度快重置了、还剩不少，建议先用（沙漏是强调色：提示，不是警告）
    private var useFirstBadge: some View {
        HStack(spacing: 3) {
            Image(systemName: "hourglass").foregroundStyle(Color.accentColor)
            Text("建议先用")
        }
        .sceneBadge()
    }

    /// 读屏念的一整句：「工作，Claude 账号，Ultra 套餐，在用，Claude App 开着 · 终端里在用，已 2 小时」
    private func label(_ a: AccountStatus, _ m: AccountMood) -> String {
        var parts = [Texts.nonEmpty(a.name) ?? "账号", a.isAPI ? "第三方 API" : Texts.department(a.tool)]
        if !a.plan.isEmpty { parts.append(Texts.planName(a.plan)) }
        parts.append(Texts.mood(m, now: now))
        if m.nearLimit { parts.append("额度即将用完") }
        if m.busy, a.appOpen || a.cliSessions > 0 { parts.append(Texts.working(a, now: now)) }
        if let f = m.useFirst { parts.append(Texts.useFirst(f, now: now)) }
        // 一次都不能重置的不念（Mac App 的 resetSummary 也不说）
        if let rc = a.usage?.resetCredits, (rc.availableCount ?? 0) > 0 { parts.append(Texts.resetCredits(rc, now: now)) }
        return parts.joined(separator: "，")
    }
}

private extension View {
    /// 插画角上的小胶囊：文字灰色，不抢插画（颜色只表达状态）
    func sceneBadge() -> some View {
        font(.caption2.weight(.semibold))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Capsule().fill(.background.opacity(0.85)))
            .overlay(Capsule().strokeBorder(Surface.hairline, lineWidth: 0.5))
            .padding(4)
    }
}

/// 「额度用完了，16:00 恢复 · 即将用完」（新建会话里挑账号的那一行）：灰字，和 Mac App 一样不给状态上色（绿只表示在用，
/// 颜色在头像、记号上）；给了 prefix 的（第三方 API 账号）前面是「第三方 API · 」
struct MoodLine: View {
    let mood: AccountMood
    var prefix: String?
    let now: Double

    var body: some View {
        let state = Text(Self.text(mood, now: now)).foregroundStyle(.secondary)
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
        if m.nearLimit { s += " · 即将用完" }
        if m.useFirst != nil { s += " · 建议先用" }
        return s
    }

}
