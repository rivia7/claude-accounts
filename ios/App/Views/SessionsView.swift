import CcaRemoteKit
import SwiftUI

/// 会话列表：一台电脑一组（照配对的先后），组里是它上面这台手机能接入的会话。看得见时 15 秒查一次，下拉也查
struct SessionsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        TimelineView(.periodic(from: .now, by: 30)) { context in
            let now = context.date.timeIntervalSince1970
            List {
                if model.isDemo {
                    Section {
                        Banner(systemImage: "sparkles", tint: .accentColor, title: "演示模式",
                               message: "这里的电脑和会话是模拟的，不联网。") {
                            Button("退出演示") { model.leaveDemo() }.buttonStyle(.bordered)
                        }
                        .listRowInsets(EdgeInsets())
                        .listRowBackground(Color.clear)
                    }
                }
                ForEach(model.peers) { p in
                    MachineSection(peer: p, listing: model.listing(p), now: now)
                }
                Section {
                    footer(now: now)
                        .listRowBackground(Color.clear)
                }
            }
            .listStyle(.insetGrouped)
        }
        .navigationTitle("远程会话")
        .refreshable { await model.fetch() }
        .toolbar { PageToolbar(mark: { RemoteMark(size: 22) }) }
    }

    private func footer(now: Double) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            if model.loading, model.fetchedAt == nil {
                ProgressView()
            } else if let at = model.fetchedAt {
                Text(Texts.agoPhrase(now - at.timeIntervalSince1970) + "查询").monospacedDigit()
            }
            Text("只有电脑上用 cca 启动、且打开了「之后启动的会话可以重新接入」的会话才会显示在这里。断开后，会话仍在电脑上运行。")
        }
        .font(.footnote)
        .foregroundStyle(.secondary)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// 一台电脑：标题行是它的名字（和 Mac App 的 MachineLine 一样：电脑图标、名字、标签），下面是会话
private struct MachineSection: View {
    @Environment(AppModel.self) private var model
    let peer: Peer
    let listing: PeerListing?
    let now: Double

    var body: some View {
        Section {
            if let l = listing {
                if l.gone {
                    Label("对方已取消配对，需要重新配对才能继续使用。", systemImage: "info.circle")
                        .font(.callout).foregroundStyle(.secondary)
                    Button("移除", role: .destructive) { model.unpair(peer) }
                } else if let f = l.failure {
                    Label(Texts.listFailure(f.code), systemImage: "exclamationmark.triangle")
                        .font(.callout).foregroundStyle(.orange)
                } else {
                    if l.sessions.isEmpty {
                        Text(l.launcher == nil ? "暂无可接入的会话。在电脑的终端里用 cca 启动一个（例如 cca @账号），就会显示在这里。"
                             : "暂无可接入的会话。点「新建会话」在电脑上启动一个，或在电脑的终端里用 cca 启动。")
                            .font(.callout).foregroundStyle(.secondary)
                    } else {
                        ForEach(l.sessions) { s in
                            SessionRow(peer: peer, session: s, now: now)
                        }
                    }
                    if let launcher = l.launcher {
                        LaunchRow(peer: peer, launcher: launcher)
                    }
                }
            } else {
                HStack { ProgressView(); Text("正在查询…").foregroundStyle(.secondary) }.font(.callout)
            }
        } header: {
            HStack(spacing: 8) {
                Image(systemName: "desktopcomputer").foregroundStyle(.secondary).accessibilityHidden(true)
                Text(peer.name).font(.subheadline.weight(.semibold)).foregroundStyle(.primary).textCase(nil).lineLimit(1)
                if let team = peer.team { Tag(text: team) }
            }
        }
    }
}

/// 「新建会话」：那台电脑让这台手机远程启动会话时才有（它的启动器在服务器上登记着），点了弹出挑模板的那一页。启动器不在线时点不了
private struct LaunchRow: View {
    @Environment(AppModel.self) private var model
    let peer: Peer
    let launcher: HostLauncher

    var body: some View {
        Button {
            model.launchFor = peer
        } label: {
            HStack(spacing: 12) {
                Label("新建会话", systemImage: "plus.circle")
                    .foregroundStyle(launcher.online ? Color.accentColor : Color.secondary)
                Spacer(minLength: 6)
                if !launcher.online {
                    Text("离线").font(.caption).foregroundStyle(.secondary)
                } else if launcher.info.templates.isEmpty {
                    Text("暂无模板").font(.caption).foregroundStyle(.secondary)
                }
            }
        }
        .disabled(!launcher.online)
        .accessibilityHint(launcher.online ? "选择一个模板，在电脑上启动新会话" : "电脑暂时没有连接服务器")
    }
}

/// 一个会话：状态点、名字、在哪个文件夹、开了多久；点一下接入（那台电脑只让看的只能看），长按可以选只看。账号的那一页也用它
struct SessionRow: View {
    @Environment(AppModel.self) private var model
    let peer: Peer
    let session: RemoteSession
    let now: Double

    var body: some View {
        let detail = Texts.sessionDetail(session, now: now)
        Button {
            model.attach(session, on: peer, readOnly: false)
        } label: {
            HStack(spacing: 12) {
                StatusDot(kind: session.online ? .on : .off)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(Texts.sessionTitle(session)).font(.body.weight(.medium)).foregroundStyle(.primary).lineLimit(1)
                        if session.info.viewOnly { Tag(text: "只看", systemImage: "eye") }
                    }
                    if !detail.isEmpty {
                        Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle).monospacedDigit()
                    }
                }
                Spacer(minLength: 6)
                if !session.online {
                    Text("离线").font(.caption).foregroundStyle(.secondary)
                } else {
                    Image(systemName: "chevron.right").font(.caption.weight(.semibold)).foregroundStyle(.tertiary)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!session.online)
        .contextMenu {
            if session.online {
                if !session.info.viewOnly {
                    Button { model.attach(session, on: peer, readOnly: false) } label: { Label("接入", systemImage: "terminal") }
                }
                Button { model.attach(session, on: peer, readOnly: true) } label: { Label("只看，不操作", systemImage: "eye") }
            }
        }
        .accessibilityHint(session.online ? (session.info.viewOnly ? "打开终端查看这个会话" : "打开终端，查看和操作这个会话") : "电脑离线")
    }
}
