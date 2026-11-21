import SwiftUI

/// 按 Team 筛选账号的菜单：侧栏、总览、菜单栏面板各放一个，改的是同一个选择（`AppModel.teamFilter`）。只在加入了 Team 时放出来；
/// 只管显示，账号表、启动器、排序都不跟着变。
struct TeamFilterMenu: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Picker(selection: Binding(get: { model.activeTeamFilter }, set: { model.teamFilter = $0 })) {
            ForEach(model.teamFilterOptions, id: \.self) { f in
                Text(model.teamFilterTitle(f)).tag(f)
            }
        } label: {
            Text("按 Team 筛选")
        }
        .pickerStyle(.menu)
        .labelsHidden()
        .fixedSize()
        .help("只显示这个 Team 给的账号（只管显示：账号表和启动器不变）")
        .accessibilityLabel("按 Team 筛选账号")
    }
}

/// 「Team」（PROTOCOL 4.14）：这台设备在的 Team（状态、给了它几个账号）、粘贴邀请链接加入、取配置、退出；管理员的 Team 展开是设备
/// （发邀请、定它能用哪些账号、移除）和各个账号走哪个节点。状态、设备、账号都照 cca 给的（`config.teams`、`team devices --json`），
/// 不在界面里重算；加入是提问会话，退出、移除由 cca 先问同意（对话框里的话是 cca 给的）
struct TeamsCard: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Card("Team", systemImage: "person.3") {
            VStack(alignment: .leading, spacing: 14) {
                Text("一个 Team 是一组设备加一份配置：哪些账号、各走哪个节点，由管理员的设备维护，存在这个 Team 的同步服务器上。管理员给每台设备发一个邀请链接，定它能用哪些账号；加入以后这些账号自动加进账号表，出口节点由管理员固定。服务器上只有密文。")
                    .font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                ForEach(model.teams) { t in
                    Divider()
                    TeamBlock(team: t)
                }
                if !model.teams.isEmpty { Divider() }
                HStack(spacing: 10) {
                    GlassRow {
                        SectionAction("粘贴邀请链接…", systemImage: "link", prominent: model.teams.isEmpty) { model.joinTeam() }
                            .help("管理员给这台设备发的邀请链接，贴到这里加入（一个邀请只能加入一台设备）")
                        if !model.teams.isEmpty {
                            SectionAction("取配置", systemImage: "arrow.clockwise") { model.syncTeams() }
                                .help("各个 Team 现在取一次配置（平时每小时自动取）：有新的就马上照它改账号表")
                        }
                    }
                    Spacer(minLength: 8)
                    if model.teamBusy { ProgressView().controlSize(.small) }
                }
                .disabled(model.flow != nil || model.teamBusy)
                Text("要自己建一个 Team（当管理员）：在终端里运行 cca team new 名字，照它说的部署服务器、接上，再回到这里发邀请。")
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// 一个 Team：名字、这台设备是不是管理员、状态、给了它几个账号；管理员的能展开「设备和账号」
private struct TeamBlock: View {
    @Environment(AppModel.self) private var model
    let team: TeamEntry

    private var expanded: Bool { model.teamExpanded.contains(team.id) }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text(nonEmpty(team.name) ?? String(team.id.prefix(8))).font(.body.weight(.medium)).textSelection(.enabled)
                if team.isAdmin { Tag(text: "管理员") }
                Spacer(minLength: 8)
                if team.isAdmin, team.state != .local {
                    RowAction(expanded ? "收起" : "设备和账号", systemImage: expanded ? "chevron.up" : "person.2.badge.gearshape") {
                        model.toggleTeam(team)
                    }
                    .help("这个 Team 里的设备（发邀请、定它能用哪些账号、移除）和各个账号走哪个节点")
                }
                Menu {
                    Button(team.isAdmin ? "删掉这个 Team…" : "退出这个 Team…", role: .destructive) { model.leaveTeam(team) }
                } label: {
                    Label("更多", systemImage: "ellipsis.circle")
                }
                .menuStyle(.button)
                .buttonStyle(.glass)
                .fixedSize()
                .disabled(model.flow != nil || model.teamBusy)
                .help(team.isAdmin ? "删掉：这台设备上的根密钥跟着没了，这个 Team 再没有谁管得了（先问一句）"
                      : "退出：这个 Team 给的账号移出账号表，登录都留着（先问一句）")
            }
            InfoRow(Text(Texts.teamState(team)), systemImage: Self.symbol(team.state), tint: Self.tint(team.state)) { EmptyView() }
            InfoRow(Text(Texts.teamSummary(team)), systemImage: "person.crop.circle") { EmptyView() }
            if expanded { TeamAdminSection(team: team) }
        }
    }

    static func symbol(_ s: TeamState) -> String {
        switch s {
        case .ok: return "checkmark.circle"
        case .pending, .local: return "clock"
        default: return "exclamationmark.triangle"
        }
    }

    static func tint(_ s: TeamState) -> Color? {
        switch s {
        case .ok: return .green
        case .pending, .local: return nil
        default: return .orange
        }
    }
}

/// 管理员展开的那一块：设备（这台在前）、发邀请、账号走哪个节点。问到的是 `team devices --json`（先和服务器对一遍）
private struct TeamAdminSection: View {
    @Environment(AppModel.self) private var model
    let team: TeamEntry

    var body: some View {
        TimelineView(.periodic(from: .now, by: 30)) { context in
            let now = context.date.timeIntervalSince1970
            let snap = model.teamSnapshot(team)
            let loading = model.teamLoading.contains(team.id)
            let accounts = snap?.doc?.accountList ?? []
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 10) {
                    Text(snap?.doc.map { "设备（\($0.devices.filter(\.state.active).count) 台）" } ?? "设备").font(.headline)
                    Spacer(minLength: 8)
                    if loading, snap?.fetchedEpoch == nil {
                        ProgressView().controlSize(.small)
                    } else if let at = snap?.fetchedEpoch {
                        Text(Texts.agoPhrase(now - at) + "查询").font(.caption).foregroundStyle(.secondary).monospacedDigit()
                    }
                    SectionAction(loading ? "刷新中…" : "刷新", systemImage: "arrow.clockwise") { model.refreshTeamDevices(team, manual: true) }
                        .disabled(loading)
                        .help("和服务器对一遍：谁加入了、各台设备手里是第几版配置")
                }
                if let e = snap?.error {
                    Label(e, systemImage: "exclamationmark.triangle")
                        .font(.callout).foregroundStyle(.orange)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let doc = snap?.doc {
                    ForEach(doc.devices.filter(\.state.active)) { d in
                        TeamDeviceLine(team: team, device: d, accounts: accounts, now: now)
                    }
                    let gone = doc.devices.filter { !$0.state.active }
                    if !gone.isEmpty {
                        Text("已经不在的：" + gone.map { "\($0.label)（\(Texts.teamDeviceDetail($0, accounts: accounts.count, now: now))）" }.joined(separator: "、"))
                            .font(.caption).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    TeamInviteForm(team: team, accounts: accounts)
                    if !accounts.isEmpty {
                        Divider()
                        Text("账号走哪个节点").font(.headline)
                        ForEach(accounts) { a in
                            TeamAccountLine(team: team, account: a, nodes: Self.nodes(doc, local: model.state?.config?.nodes ?? [], current: a.node))
                        }
                        Text("改了以后各台设备下次取配置时跟着换（每小时一次，或者在那台设备上点「取配置」）。加账号、导入现成的映射在终端里：cca team map、cca team import。")
                            .font(.caption).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                } else if !loading, snap?.error == nil {
                    Text("还没问过服务器").font(.callout).foregroundStyle(.secondary)
                }
            }
            .padding(.top, 4)
        }
    }

    /// 能选的节点：这个 Team 带着定义的、这台设备上有的，加上现在写着的那个（免得选单里没有它）；去重，保持先后
    static func nodes(_ doc: TeamDevicesDoc, local: [String], current: String) -> [String] {
        var seen = Set<String>(), out: [String] = []
        for n in doc.nodes + local + [current] where !n.isEmpty && seen.insert(n).inserted { out.append(n) }
        return out
    }
}

/// 一台设备：名字（管理员起的；它自己报的不一样时带上）、状态、给了它几个账号；右边改它能用哪些账号、移除（管理员自己这台不能移除）
private struct TeamDeviceLine: View {
    @Environment(AppModel.self) private var model
    let team: TeamEntry
    let device: TeamDevice
    let accounts: [TeamAccount]
    let now: Double

    private var name: String {
        guard let n = nonEmpty(device.name), n != device.label else { return device.label }
        return "\(device.label)（\(n)）"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            MachineLine(name: name, detail: Texts.teamDeviceDetail(device, accounts: accounts.count, now: now)) {
                if device.me { Tag(text: "这台 Mac") }
                if device.problem != nil { Tag(text: "要处理", color: .orange) }
            } trailing: {
                HStack(spacing: 8) {
                    Menu {
                        Toggle("全部账号（以后加进 Team 的也给）", isOn: Binding(get: { device.grantsAll }, set: { model.setTeamGrantAll(team, device, $0) }))
                        if !accounts.isEmpty { Divider() }
                        ForEach(accounts) { a in
                            Toggle("\(Texts.product(a.tool)) · \(a.email)",
                                   isOn: Binding(get: { device.has(a) }, set: { model.setTeamGrant(team, device, a, on: $0, all: accounts) }))
                        }
                    } label: {
                        Label("账号", systemImage: "person.crop.circle.badge.checkmark")
                    }
                    .menuStyle(.button)
                    .buttonStyle(.glass)
                    .controlSize(.small)
                    .fixedSize()
                    .help("这台设备能用这个 Team 里的哪些账号：改了马上发布，它下次取配置时生效")
                    if !device.me {
                        RowAction(device.state == .invited ? "撤回…" : "移除…", systemImage: "xmark.circle") { model.revokeTeamDevice(team, device) }
                            .help(device.state == .invited ? "撤回这个还没用的邀请（先问一句）"
                                  : "从这个 Team 里移除这台设备：它的密钥作废、给它的配置删掉；它已经拿到的节点凭证收不回来（先问一句）")
                    }
                }
                .disabled(model.teamBusy || model.flow != nil)
            }
            if let p = device.problem {
                Label(p, systemImage: "exclamationmark.triangle")
                    .font(.caption).foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// 发邀请：给设备起个名字、定它能用哪些账号（默认全部），邀请链接复制到剪贴板（不显示在界面上：它是那台设备的密钥）
private struct TeamInviteForm: View {
    @Environment(AppModel.self) private var model
    let team: TeamEntry
    let accounts: [TeamAccount]

    private var draft: TeamInviteDraft { model.teamInvite.team == team.id ? model.teamInvite : TeamInviteDraft(team: team.id) }

    private func edit(_ change: (inout TeamInviteDraft) -> Void) {
        var d = draft
        change(&d)
        model.teamInvite = d
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Divider()
            Text("邀请一台设备").font(.headline)
            HStack(spacing: 10) {
                TextField("给这台设备起个名字（比如 小王的 MacBook）", text: Binding(get: { draft.label }, set: { v in edit { $0.label = v } }))
                    .textFieldStyle(.roundedBorder)
                    .frame(maxWidth: 300)
                Menu {
                    Toggle("全部账号（以后加进 Team 的也给）", isOn: Binding(get: { draft.all }, set: { v in edit { $0.all = v } }))
                    if !accounts.isEmpty { Divider() }
                    ForEach(accounts) { a in
                        Toggle("\(Texts.product(a.tool)) · \(a.email)", isOn: Binding(get: { draft.all || draft.picked.contains(a.spec) }, set: { v in
                            edit { d in
                                if d.all { d.picked = Set(accounts.map(\.spec)) }   // 从全部里去掉一个：改成只给剩下的
                                d.all = false
                                if v { d.picked.insert(a.spec) } else { d.picked.remove(a.spec) }
                            }
                        }))
                    }
                } label: {
                    Label(draft.all ? "全部账号" : "\(draft.picked.count) 个账号", systemImage: "person.crop.circle.badge.checkmark")
                }
                .menuStyle(.button)
                .buttonStyle(.glass)
                .fixedSize()
                .help("这台设备能用哪些账号（之后还能改）")
                SectionAction("复制邀请链接", systemImage: "doc.on.doc", prominent: true) { model.sendTeamInvite(team) }
                    .disabled(draft.label.trimmingCharacters(in: .whitespaces).isEmpty)
                    .help("生成一个邀请，链接放进剪贴板（60 秒后清掉）：发给那台设备的主人，对方「粘贴邀请链接…」加入")
            }
            .disabled(model.teamBusy || model.flow != nil)
            Text("一个邀请只能加入一台设备，7 天后过期。链接就是那台设备的密钥：当密码传，别贴到群里。")
                .font(.caption).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// Team 的配置里的一个账号：走哪个节点（管理员定的）
private struct TeamAccountLine: View {
    @Environment(AppModel.self) private var model
    let team: TeamEntry
    let account: TeamAccount
    let nodes: [String]

    var body: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 1) {
                Text(nonEmpty(account.alias).map { "\($0)（\(account.email)）" } ?? account.email).lineLimit(1).truncationMode(.middle)
                Text(Texts.product(account.tool)).font(.caption).foregroundStyle(.secondary)
            }
            Spacer(minLength: 8)
            Picker("节点", selection: Binding(get: { account.node }, set: { model.mapTeamAccount(team, account, node: $0) })) {
                ForEach(nodes, id: \.self) { Text($0).tag($0) }
            }
            .labelsHidden()
            .fixedSize()
            .disabled(model.teamBusy || model.flow != nil)
            .help("这个账号在各台设备上走哪个节点")
        }
    }
}
