import SwiftUI

/// 办公室（App 的首页）：每个账号是一位员工，坐在自己的工位上，按组排好（Claude 组、ChatGPT 组、外包）。
/// 一眼看出谁在忙、谁在休息、谁没来、谁出了毛病；点工位进这个账号的页面，右键和侧栏的账号一样。
/// 状态全按 cca 给的（Office / Employee，和账号页那句话同一份优先级），“休息到几点”到点就醒：每 30 秒按时间重算。
struct OfficePage: View {
    @Environment(AppModel.self) private var model
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ScrollView {
            TimelineView(.periodic(from: .now, by: 30)) { ctx in
                let now = ctx.date.timeIntervalSince1970
                let office = model.office(now: now)
                VStack(alignment: .leading, spacing: 26) {
                    header(office)
                    TipCard(id: "office-page", title: "这一页怎么看",
                            message: "每个账号是一位员工，坐在自己的工位上。屏幕在滚代码、聊天，说明它正在被使用；趴着睡觉、屏幕黑了，是额度用完了，屏幕上写着几点回来；椅子空着是还没登录，坐着陌生人是登错了号。名牌越亮套餐越高，金色皇冠是最高档。点一个工位看详情，右键可以直接打开。")
                    if model.state == nil {
                        HStack(spacing: 8) {
                            ProgressView().controlSize(.small)
                            Text("正在读取账号…").foregroundStyle(.secondary)
                        }
                    } else {
                        ForEach(office.rooms) { room in
                            RoomSection(room: room, now: now, animate: model.mainVisible && !reduceMotion)
                        }
                    }
                }
                .padding(.top, 24)
                .padding(.bottom, 36)
                .pageColumn()
            }
        }
        .navigationTitle("办公室")
    }

    private func header(_ o: Office) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                Text("办公室").font(.system(size: 24, weight: .semibold))
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
                    Text("\(room.people.count) 人").font(.callout).foregroundStyle(.secondary)
                }
                Button {
                    model.runInTerminal(add)
                } label: {
                    Label(add.title + "…", systemImage: "plus")
                }
                .buttonStyle(.borderless)
                .help("在终端里运行 \(add.display)")
            }
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 228, maximum: 340), spacing: 16)], alignment: .leading, spacing: 16) {
                ForEach(room.people) { e in
                    DeskCard(employee: e, now: now, animate: animate)
                }
                if room.people.isEmpty {
                    EmptyDesk(title: room.department == .chatgpt ? "还没有 ChatGPT 账号" : "还没有账号", action: add.title + "…") {
                        model.runInTerminal(add)
                    }
                }
            }
        }
    }
}

/// 一个工位：插画、名字和套餐徽章、现在怎么样、两行小用量。整张卡片是按钮（打开账号页），右键和侧栏一样。
private struct DeskCard: View {
    @Environment(AppModel.self) private var model
    @Environment(Guide.self) private var guide
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let employee: Employee
    let now: Double
    let animate: Bool
    /// 悬停（命令行工具编不了 @State 宏：State 类型直接用）
    private let hover = State(initialValue: false)

    var body: some View {
        let e = employee, a = employee.account
        let label = Texts.deskLabel(e, appLabel: model.appLabel(a.tool), now: now)
        let on = hover.wrappedValue
        let shape = RoundedRectangle(cornerRadius: 18, style: .continuous)
        Button {
            model.show(a.ref)
        } label: {
            VStack(alignment: .leading, spacing: 10) {
                DeskScene(employee: e, animate: animate, now: now)
                    .overlay(alignment: .topLeading) { workBadge(e, a) }
                    .overlay(alignment: .topTrailing) { flagBadge(e, a) }
                info(a, e)
            }
            .padding(12)
            .background(shape.fill(on ? Color.primary.opacity(0.08) : Surface.tile))
            .overlay {
                if a.isPremium {
                    shape.strokeBorder(TierStyle.goldStroke, lineWidth: 1.5)
                } else {
                    shape.strokeBorder(Color.primary.opacity(on ? 0.16 : 0), lineWidth: 1)
                }
            }
            .contentShape(shape)
            .scaleEffect(on && !reduceMotion ? 1.01 : 1)
            .animation(reduceMotion ? nil : .snappy(duration: 0.18), value: on)
        }
        .buttonStyle(.plain)
        .onHover { hover.wrappedValue = $0 }
        .contextMenu { AccountMenu(account: a) }
        .help(label)
        .accessibilityLabel(label)
        .accessibilityHint("打开这个账号的页面")
    }

    @ViewBuilder
    private func info(_ a: Account, _ e: Employee) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                Text(a.displayName).font(.callout.weight(.semibold)).lineLimit(1).truncationMode(.middle)
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
        case .wrongPerson: Circle().fill(Color.orange).frame(width: 6, height: 6)
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
            .font(.caption2.weight(.semibold))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Capsule().fill(.background.opacity(0.85)))
            .overlay(Capsule().strokeBorder(Surface.hairline, lineWidth: 0.5))
            .help(Texts.working(app: e.app, appLabel: model.appLabel(a.tool), sessions: e.sessions, since: e.since, now: now))
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

/// 空着的工位（这个组还没有账号）：整张是按钮，点了在终端里添加。
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
