import CcaRemoteKit
import SwiftUI

/// 「共享与同步」（Mac App 的 SharingPage、TeamsCard）：配对过的电脑在哪个 Team 里、在里面怎么样（配置同步了没有、是不是管理员、
/// 分给它几个账号）。这台手机经这个 Team 的服务器和电脑配对、接入会话、看账号的状态。一个 Team 一块，里面是这个 Team 里的几台电脑；
/// 照那台电脑上的 cca 给的说（config.teams 里这个配对所在的那一项，跟账号的状态一起放上来），这里只排版。加入、新建、邀请、退出都在电脑上做
struct SharingView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let found = SharingGroups(model)
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                Text("电脑所在的 Team：管理员定好每个账号走哪个节点、分给哪些设备；设备统计、折合金额经这个 Team 的服务器在几台设备之间同步。这台手机也经这台服务器和电脑配对、接入会话。")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                ForEach(found.teams) { TeamCard(group: $0) }
                if !found.others.isEmpty { OtherMachines(rows: found.others) }
                Text("加入或新建 Team、邀请设备、分配账号，请在电脑上的 cca App 的「共享与同步」中操作（或在终端运行 cca team）。")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.horizontal, 20)
            .padding(.top, 8)
            .padding(.bottom, 30)
            .pageColumn()
        }
        .navigationTitle("共享与同步")
        .refreshable { await model.fetch() }
        .toolbar { PageToolbar(mark: { SharingMark(size: 22) }) }
    }
}

/// 一个 Team 里的一台电脑：配对的那台、它放上来的这个 Team 的状态
struct TeamMachine: Identifiable {
    let peer: Peer
    let team: HostTeam
    var id: String { peer.pid }
}

/// 一个 Team：同名、同一台服务器的算一个（几台电脑在同一个 Team 里）
struct TeamGroup: Identifiable {
    let name: String
    let host: String
    var machines: [TeamMachine]
    var id: String { name + "\u{0}" + host }
}

/// 按 Team 分好的电脑（照配对的先后）；没放上来 Team 的状态的电脑另列，说为什么
struct SharingGroups {
    var teams: [TeamGroup] = []
    var others: [(peer: Peer, text: String)] = []

    @MainActor
    init(_ model: AppModel) {
        for p in model.peers {
            guard let l = model.listing(p) else {
                others.append((p, "正在查询…"))
                continue
            }
            if l.gone {
                others.append((p, "对方已取消配对，需要重新配对才能继续使用。"))
            } else if let st = l.status {
                if st.off {
                    others.append((p, "电脑已不允许这台手机接入。"))
                } else if let t = st.team {
                    let key = t.name + "\u{0}" + t.host
                    if let i = teams.firstIndex(where: { $0.id == key }) {
                        teams[i].machines.append(TeamMachine(peer: p, team: t))
                    } else {
                        teams.append(TeamGroup(name: t.name, host: t.host, machines: [TeamMachine(peer: p, team: t)]))
                    }
                } else {
                    others.append((p, "电脑上的 cca 没有上传 Team 的状态：更新到最新版并查询一次用量后就会显示。"))
                }
            } else if let f = l.failure {
                others.append((p, Texts.listFailure(f.code)))
            } else {
                others.append((p, l.statusServed ? "电脑还没上传状态：把电脑上的 cca 更新到最新版并查询一次用量后就会显示。"
                                  : "这台电脑使用的服务器还不支持在手机上查看状态，需要管理员更新服务器。"))
            }
        }
    }
}

/// 一个 Team 一块（Mac App 的 TeamBlock）：名字、服务器，下面是在这个 Team 里的几台电脑
private struct TeamCard: View {
    let group: TeamGroup

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                Label(Texts.nonEmpty(group.name) ?? "Team", systemImage: "person.3").font(.headline)
                if !group.host.isEmpty {
                    Text(group.host).font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                        .accessibilityLabel("服务器 " + group.host)
                }
            }
            ForEach(group.machines) { m in
                Divider()
                MachineInTeam(machine: m)
            }
        }
        .padding(18)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Surface.tile))
    }
}

/// 一台电脑在这个 Team 里：名字和是不是管理员，状态（同步了没有）、服务器要不要更新、在里面叫什么和分给它几个账号
private struct MachineInTeam: View {
    let machine: TeamMachine

    var body: some View {
        let t = machine.team
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Image(systemName: "desktopcomputer").foregroundStyle(.secondary).accessibilityHidden(true)
                Text(machine.peer.name).font(.body.weight(.medium)).lineLimit(1)
                if let role = Texts.teamRole(t) { Tag(text: role) }
            }
            .accessibilityElement(children: .combine)
            InfoLine(text: Texts.teamState(t), systemImage: Self.symbol(t.state), tint: Self.tint(t.state))
            if let u = Texts.teamServerUpdate(t) {
                InfoLine(text: u, systemImage: "arrow.up.circle", tint: .orange)
            }
            InfoLine(text: Texts.teamSummary(t, host: false), systemImage: "person.crop.circle")
        }
    }

    static func symbol(_ s: String) -> String {
        switch s {
        case "ok": return "checkmark.circle"
        case "pending", "local": return "clock"
        default: return "exclamationmark.triangle"
        }
    }

    static func tint(_ s: String) -> Color? {
        switch s {
        case "ok": return .green
        case "pending", "local": return nil
        default: return .orange
        }
    }
}

/// 一行说明（Mac App 的 InfoRow）：前面一个小图标（有颜色的是状态），字灰色（要处理的用状态的颜色）
struct InfoLine: View {
    let text: String
    let systemImage: String
    var tint: Color?

    var body: some View {
        Label {
            Text(text).foregroundStyle(tint == .orange ? Color.orange : Color.secondary).fixedSize(horizontal: false, vertical: true)
        } icon: {
            Image(systemName: systemImage).foregroundStyle(tint ?? .secondary)
        }
        .font(.callout)
    }
}

/// 没放上来 Team 的状态的电脑：说为什么
private struct OtherMachines: View {
    let rows: [(peer: Peer, text: String)]

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label("其他电脑", systemImage: "desktopcomputer").font(.headline)
            ForEach(rows, id: \.peer.pid) { r in
                Divider()
                VStack(alignment: .leading, spacing: 6) {
                    MachineHeader(peer: r.peer)
                    Text(r.text).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .padding(18)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Surface.tile))
    }
}
