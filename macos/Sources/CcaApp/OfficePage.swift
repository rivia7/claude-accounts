import SwiftUI

/// 办公室（App 的首页）：每个账号是一位员工，坐在自己的工位上，按组排好（Claude 组、ChatGPT 组、外包）。
/// 一眼看出谁在忙、谁在休息、谁没来、谁出了毛病；点工位进这个账号的页面，右键和侧栏的账号一样，拖到别的工位上调整顺序。
/// 状态全按 cca 给的（Office / Employee，和账号页那句话同一份优先级），“休息到几点”到点就醒：每 30 秒按时间重算。
struct OfficePage: View {
    @Environment(AppModel.self) private var model
    @Environment(\.motionAllowed) private var motionAllowed

    var body: some View {
        ScrollView {
            TimelineView(.periodic(from: .now, by: 30)) { ctx in
                let now = ctx.date.timeIntervalSince1970
                let office = model.office(now: now)
                VStack(alignment: .leading, spacing: 26) {
                    header(office)
                    TipCard(id: "office-page", title: "这一页怎么看",
                            message: "每个账号一张卡片，图里是它现在的状态：屏幕在动是在用；趴在桌上是额度用完了，屏幕上写着几点恢复；椅子空着是还没登录，灰色的人是登录的不是这个账号。左下角的小票是还能重置几次额度，右下角的沙漏是建议先用（本周额度快重置了、还剩不少）。点卡片看详情，右键直接打开，拖动调整顺序。")
                    if model.state == nil {
                        HStack(spacing: 8) {
                            ProgressView().controlSize(.small)
                            Text("正在读取账号…").foregroundStyle(.secondary)
                        }
                    } else {
                        ForEach(office.rooms) { room in
                            RoomSection(room: room, now: now, animate: motionAllowed)
                        }
                    }
                }
                .padding(.top, 24)
                .padding(.bottom, 36)
                .pageColumn()
            }
        }
        .navigationTitle("总览")
    }

    private func header(_ o: Office) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                Text("总览").font(.system(size: 24, weight: .semibold))
                if model.stateLoading { ProgressView().controlSize(.small) }
            }
            Text(Texts.officeSummary(o.census))
                .font(.callout)
                .foregroundStyle(.secondary)
        }
    }
}

/// 一个组：标题（几个人、添加账号）和一排排工位。
private struct RoomSection: View {
    @Environment(AppModel.self) private var model
    let room: Office.Room
    let now: Double
    let animate: Bool

    var body: some View {
        let add = model.addHandoff(room.department)
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader(Texts.department(room.department)) {
                if !room.people.isEmpty {
                    Text("\(room.people.count) 个").font(.callout).foregroundStyle(.secondary)
                }
                SectionAction(add.title + "…", systemImage: "plus") { model.run(add) }
            }
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 228, maximum: 340), spacing: 16)], alignment: .leading, spacing: 16) {
                ForEach(room.people) { e in
                    DeskCard(employee: e, now: now, animate: animate)
                }
                if room.people.isEmpty {
                    EmptyDesk(title: room.department == .chatgpt ? "还没有 ChatGPT 账号"
                                : room.department == .antigravity ? "还没有 Antigravity 账号" : "还没有账号", action: add.title + "…") {
                        model.run(add)
                    }
                }
            }
        }
    }
}

/// 一个工位：插画、名字和套餐徽章、现在怎么样、几行小用量（5 小时、本周、Claude 的 Fable）。整张卡片是按钮（打开账号页），右键和侧栏一样。
private struct DeskCard: View {
    @Environment(AppModel.self) private var model
    @Environment(Guide.self) private var guide
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let employee: Employee
    let now: Double
    let animate: Bool
    /// 悬停（命令行工具编不了 @State 宏：State 类型直接用）
    private let hover = State(initialValue: false)
    /// 别的卡片正拖到这张上面
    private let targeted = State(initialValue: false)

    var body: some View {
        let e = employee, a = employee.account
        let resets = model.usage(for: a)?.resetCredits
        let label = Texts.deskLabel(e, appLabel: model.appLabel(a.tool), now: now, resets: resets)
        let on = hover.wrappedValue
        let shape = RoundedRectangle(cornerRadius: 18, style: .continuous)
        Button {
            model.show(a.ref)
        } label: {
            VStack(alignment: .leading, spacing: 10) {
                DeskScene(employee: e, animate: animate, now: now)
                    .overlay(alignment: .topLeading) { workBadge(e, a) }
                    .overlay(alignment: .topTrailing) { flagBadge(e, a) }
                    .overlay(alignment: .bottomLeading) { resetBadge(resets) }
                    .overlay(alignment: .bottomTrailing) { useFirstBadge(e) }
                info(a, e)
            }
            .padding(12)
            // 同一排的卡片一样高：有 Fable 的多一行小条，比没登录的高一截；矮的撑到这一排最高的，多出来的空在下面
            .frame(maxHeight: .infinity, alignment: .top)
            .background(shape.fill(on ? Color.primary.opacity(0.08) : Surface.tile))
            .overlay(shape.strokeBorder(targeted.wrappedValue ? Color.accentColor : Color.primary.opacity(on ? 0.16 : 0),
                                        lineWidth: targeted.wrappedValue ? 2 : 1))
            .contentShape(shape)
            .scaleEffect(on && !reduceMotion ? 1.01 : 1)
            .animation(reduceMotion ? nil : .snappy(duration: 0.18), value: on)
        }
        .buttonStyle(.plain)
        .onHover { hover.wrappedValue = $0 }
        .contextMenu { AccountMenu(account: a) }
        .draggable(a.stableID) { DeskDragPreview(account: a) }
        .dropDestination(for: String.self) { ids, _ in
            ids.first.map { model.dropAccount($0, onto: a) } ?? false
        } isTargeted: { targeted.wrappedValue = $0 }
        .help(label)
        .accessibilityLabel(label)
        .accessibilityHint("打开这个账号的页面")
    }

    @ViewBuilder
    private func info(_ a: Account, _ e: Employee) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                Text(a.displayName).font(.callout.weight(.semibold)).lineLimit(1).truncationMode(.middle)
                if model.isOnTop(a) { OnTopMark() }
                Spacer(minLength: 4)
                if a.isAPI {
                    Text(nonEmpty(a.api?.model) ?? "第三方 API").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                } else if let p = nonEmpty(a.plan) {
                    PlanBadge(plan: p, tier: a.planTier, size: .compact)
                }
            }
            HStack(alignment: .center, spacing: 6) {
                if let act = model.activity(a.ref) {
                    ProgressView().controlSize(.mini)
                    Text(Texts.activity(act, appLabel: model.appLabel(a.tool))).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                } else {
                    moodMark(e)
                    Text(moodText(a, e)).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 4)
                if let u = model.usage(for: a), u.source != .none {
                    UsageStack(usage: u, width: 26)
                }
            }
            .frame(minHeight: 28)
            if guide.showDetails, let n = a.node {
                Text([n, a.port.map { "端口 \($0)" }, a.group].compactMap { $0 }.joined(separator: " · "))
                    .font(.caption2).foregroundStyle(.tertiary).lineLimit(1)
            }
        }
    }

    /// 外包的工位写上是哪个工具的接口
    private func moodText(_ a: Account, _ e: Employee) -> String {
        let m = Texts.mood(e, appLabel: model.appLabel(a.tool), now: now, brief: true)
        return a.isAPI ? Texts.product(a.tool) + " · " + m : m
    }

    /// 状态前面的小记号：颜色只表达状态（正常时不出现绿色以外的）
    @ViewBuilder
    private func moodMark(_ e: Employee) -> some View {
        switch e.mood {
        case .working: Circle().fill(Color.green).frame(width: 6, height: 6)
        case .absent: Circle().fill(Color.gray).frame(width: 6, height: 6)
        case .wrongPerson, .expired: Circle().fill(Color.orange).frame(width: 6, height: 6)
        case .unwell(let a): Circle().fill(a.severe ? Color.red : Color.orange).frame(width: 6, height: 6)
        case .resting: Image(systemName: "moon.zzz.fill").font(.caption2).foregroundStyle(.secondary)
        case .idle: EmptyView()
        }
    }

    /// 左上角：在用什么（桌面 App、命令行会话几个）
    @ViewBuilder
    private func workBadge(_ e: Employee, _ a: Account) -> some View {
        if e.isWorking {
            HStack(spacing: 4) {
                if e.app { Image(systemName: "macwindow") }
                if e.sessions > 0 {
                    Image(systemName: "terminal")
                    if e.sessions > 1 { Text("\(e.sessions)").monospacedDigit() }
                }
            }
            .sceneBadge()
            .help(Texts.working(app: e.app, appLabel: model.appLabel(a.tool), sessions: e.sessions, since: e.since, now: now))
        }
    }

    /// 左下角：还能重置几次额度，一张小票一次（太多了写 +N），旁边是最早什么时候过期。没有机会时不画。
    @ViewBuilder
    private func resetBadge(_ rc: ResetCredits?) -> some View {
        if let rc, let n = rc.availableCount, let summary = Texts.resetSummary(rc, now: now) {
            let drawn = min(n, 5)
            HStack(spacing: 3) {
                ForEach(0..<drawn, id: \.self) { _ in Image(systemName: "ticket.fill") }
                if n > drawn { Text("+\(n - drawn)").monospacedDigit() }
                if let epoch = rc.nextExpiresEpoch {
                    Text(Texts.expiryText(epoch: epoch, now: now)).lineLimit(1).padding(.leading, 2)
                }
            }
            .sceneBadge()
            .help(summary)
        }
    }

    /// 右下角：本周额度快重置了、还剩不少，建议先用这个账号（沙漏是强调色：提示，不是警告）
    @ViewBuilder
    private func useFirstBadge(_ e: Employee) -> some View {
        if let f = e.useFirst {
            HStack(spacing: 3) {
                Image(systemName: "hourglass").foregroundStyle(Color.accentColor)
                Text("建议先用")
            }
            .sceneBadge()
            .help(Texts.useFirst(f, now: now))
        }
    }

    /// 右上角：头条之外还要提一句的（额度快用完、泄露、纯净度低是橙色；没装 App、没检测过只是提示）
    @ViewBuilder
    private func flagBadge(_ e: Employee, _ a: Account) -> some View {
        let notes = Texts.flags(e.flags, appLabel: model.appLabel(a.tool))
        if !notes.isEmpty {
            let warn = e.flags.contains(where: \.warns)
            Image(systemName: warn ? "exclamationmark.circle.fill" : "info.circle")
                .font(.callout)
                .foregroundStyle(warn ? Color.orange : Color.secondary)
                .background(Circle().fill(.background).padding(1))
                .help(notes.joined(separator: "；"))
        }
    }

}

private extension View {
    /// 插画角上的小胶囊：文字灰色，不抢插画（颜色只表达状态）。
    func sceneBadge() -> some View {
        font(.caption2.weight(.semibold))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Capsule().fill(.background.opacity(0.85)))
            .overlay(Capsule().strokeBorder(Surface.hairline, lineWidth: 0.5))
    }
}

/// 拖动时跟着鼠标的小卡片（整张工位带动画，拖着太重）。
private struct DeskDragPreview: View {
    let account: Account

    var body: some View {
        Text(account.displayName)
            .font(.callout.weight(.medium))
            .lineLimit(1)
            .padding(.horizontal, 12)
            .padding(.vertical, 7)
            .background(Capsule().fill(.regularMaterial))
    }
}

/// 空着的工位（这个组还没有账号）：整张是按钮，点了添加账号。
private struct EmptyDesk: View {
    let title: String
    let action: String
    let run: () -> Void

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 18, style: .continuous)
        Button(action: run) {
            VStack(spacing: 8) {
                Image(systemName: "chair.lounge").font(.system(size: 28)).foregroundStyle(.tertiary)
                Text(title).font(.callout).foregroundStyle(.secondary)
                Text(action).font(.caption).foregroundStyle(.tint)
            }
            .frame(maxWidth: .infinity, minHeight: 190)
            .contentShape(shape)
            .overlay(shape.strokeBorder(style: StrokeStyle(lineWidth: 1, dash: [5, 4])).foregroundStyle(.tertiary))
        }
        .buttonStyle(.plain)
    }
}
