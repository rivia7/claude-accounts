import CcaRemoteKit
import SwiftUI

/// 会话列表：一台电脑一组（照配对的先后），组里是它上面这台手机能接入的会话。看得见时 15 秒查一次，下拉也查
struct SessionsView: View {
    @Environment(AppModel.self) private var model

    /// 「排序」：拖动右边的把手调整会话的先后（List 的编辑状态，只能挪、不能删）。不点它也能长按拖动
    private var editing: Bool { model.sortingSessions }
    private var editMode: Binding<EditMode> {
        Binding(get: { model.sortingSessions ? .active : .inactive }, set: { model.sortingSessions = $0.isEditing })
    }
    /// 有一台电脑上不止一个会话时才排得了
    private var sortable: Bool { model.peers.contains { (model.listing($0)?.sessions.count ?? 0) > 1 } }

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
                if !model.terminals.isEmpty {
                    OpenTerminalsSection()
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
            .environment(\.editMode, editMode)
        }
        .navigationTitle("远程会话")
        .refreshable { await model.fetch() }
        .toolbar {
            PageToolbar(mark: { RemoteMark(size: 22) }, extra: {
                if editing || sortable {
                    Button(editing ? "完成" : "排序") {
                        withAnimation { model.sortingSessions.toggle() }
                    }
                    .accessibilityHint(editing ? "结束排序" : "拖动会话右边的把手调整先后")
                }
            })
        }
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
                } else if l.status?.off == true, l.sessions.isEmpty {
                    // 那台电脑改成了不让这台手机接入（总览、共享与同步也这么说）
                    Label("电脑已不允许这台手机接入。", systemImage: "hand.raised")
                        .font(.callout).foregroundStyle(.secondary)
                } else {
                    if l.sessions.isEmpty {
                        Text(l.launcher == nil ? "暂无可接入的会话。在电脑的终端里用 cca 启动一个（例如 cca @账号），就会显示在这里。"
                             : "暂无可接入的会话。点「新建会话」在电脑上启动一个，或在电脑的终端里用 cca 启动。")
                            .font(.callout).foregroundStyle(.secondary)
                    } else {
                        // 照这台手机记着的先后（拖动排过的，SessionOrder）；读屏的用「上移」「下移」
                        let ordered = model.sessions(l)
                        ForEach(Array(ordered.enumerated()), id: \.element.id) { i, s in
                            SessionRow(peer: peer, session: s, now: now)
                                .moveDisabled(ordered.count < 2)
                                .accessibilityActions {
                                    if i > 0 {
                                        Button("上移") { model.moveSessions(of: l, from: IndexSet(integer: i), to: i - 1) }
                                    }
                                    if i + 1 < ordered.count {
                                        Button("下移") { model.moveSessions(of: l, from: IndexSet(integer: i), to: i + 2) }
                                    }
                                }
                        }
                        .onMove { model.moveSessions(of: l, from: $0, to: $1) }
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

/// 「新建会话」：那台电脑让这台手机远程启动会话时才有（它的启动器在服务器上登记着），点了弹出挑模板的那一页。启动器不在线、
/// 电脑上还没有模板时点不了（和 Mac App 一样）
private struct LaunchRow: View {
    @Environment(AppModel.self) private var model
    let peer: Peer
    let launcher: HostLauncher

    var body: some View {
        let usable = launcher.online && !launcher.info.templates.isEmpty
        Button {
            model.launchFor = LaunchTarget(peer: peer)
        } label: {
            HStack(spacing: 12) {
                Label("新建会话", systemImage: "plus.circle")
                    .foregroundStyle(usable ? Color.accentColor : Color.secondary)
                Spacer(minLength: 6)
                if !launcher.online {
                    Text("离线").font(.caption).foregroundStyle(.secondary)
                } else if launcher.info.templates.isEmpty {
                    Text("暂无模板").font(.caption).foregroundStyle(.secondary)
                }
            }
        }
        .disabled(!usable)
        .accessibilityHint(!launcher.online ? "电脑暂时没有连接服务器" : usable ? "选择一个模板，在电脑上启动新会话" : "电脑上还没有模板")
    }
}

/// 一个会话：状态点、名字、在哪个文件夹、开了多久；点一下接入（那台电脑只让看的只能看），已经开着它的终端的换过去（「已接入」，和
/// Mac App 的「切换」一样）；长按可以选只看、换一种接法、断开，远程启动的会话还能结束（先问一句，列表里左滑也行）。账号的那一页也用它
struct SessionRow: View {
    @Environment(AppModel.self) private var model
    @Environment(\.editMode) private var editMode
    let peer: Peer
    let session: RemoteSession
    let now: Double
    @State private var confirmingEnd = false

    var body: some View {
        let detail = Texts.sessionDetail(session, now: now)
        let editing = editMode?.wrappedValue.isEditing == true
        // 这个会话在 App 里开着、还接着的终端：点一下换过去，不再接一次
        let open = model.openTerminal(for: session.sid, on: peer.pid).flatMap { $0.isClosed ? nil : $0 }
        let ending = model.endingSessions.contains(session.sid)
        Button {
            // 排序的时候点了不接入（拖的是右边的把手）
            guard !editing else { return }
            model.attach(session, on: peer, readOnly: open?.readOnly ?? false)
        } label: {
            HStack(spacing: 12) {
                StatusDot(kind: session.online ? .on : .off)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(Texts.sessionTitle(session)).font(.body.weight(.medium)).foregroundStyle(.primary).lineLimit(1)
                        if session.info.viewOnly { Tag(text: "只看", systemImage: "eye") }
                        if open != nil { Tag(text: "已接入", color: .green) }
                    }
                    if !detail.isEmpty {
                        Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle).monospacedDigit()
                    }
                }
                Spacer(minLength: 6)
                if ending {
                    ProgressView().controlSize(.small)
                    Text("正在结束…").font(.caption).foregroundStyle(.secondary)
                } else if !session.online {
                    Text("离线").font(.caption).foregroundStyle(.secondary)
                } else if !editing {
                    Image(systemName: "chevron.right").font(.caption.weight(.semibold)).foregroundStyle(.tertiary)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!session.online && !editing)
        .contextMenu {
            if session.online {
                if let open {
                    Button { model.show(open) } label: { Label("切换到它的终端", systemImage: "arrow.right.circle") }
                    if !session.info.viewOnly {
                        Button { model.attach(session, on: peer, readOnly: !open.readOnly) } label: {
                            Label(open.readOnly ? "改成可以操作（重新接入）" : "改成只看（重新接入）", systemImage: open.readOnly ? "keyboard" : "eye")
                        }
                    }
                    Button { model.closeTerminal(open) } label: {
                        Label { Text("断开"); Text("会话仍在电脑上运行") } icon: { Image(systemName: "xmark.circle") }
                    }
                } else {
                    if !session.info.viewOnly {
                        Button { model.attach(session, on: peer, readOnly: false) } label: { Label("接入", systemImage: "terminal") }
                    }
                    Button { model.attach(session, on: peer, readOnly: true) } label: { Label("只看，不操作", systemImage: "eye") }
                }
                if session.info.canEnd, !ending {
                    Button(role: .destructive) { confirmingEnd = true } label: { Label("结束会话…", systemImage: "stop.circle") }
                }
            }
        }
        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
            if session.online, session.info.canEnd, !ending {
                Button { confirmingEnd = true } label: { Label("结束", systemImage: "stop.circle") }
                    .tint(.red)
            }
        }
        .confirmationDialog("结束这个会话？", isPresented: $confirmingEnd, titleVisibility: .visible) {
            Button("结束会话", role: .destructive) { model.end(session, on: peer) }
            Button("取消", role: .cancel) {}
        } message: {
            Text(Texts.endSessionWarning(session))
        }
        .accessibilityHint(hint(open: open != nil))
    }

    private func hint(open: Bool) -> String {
        guard session.online else { return "电脑离线" }
        if open { return "已接入：切换到它的终端" }
        return session.info.viewOnly ? "打开终端查看这个会话" : "打开终端，查看和操作这个会话"
    }
}

/// 「已打开的终端」（开着终端时才有，照接入的先后）：从终端返回以后照样接着（和 Mac App 页顶那一排一样），点一个回到它的终端；
/// 左滑、长按可以断开（会话在电脑上照常运行），会话结束了、断了的是「关闭」
private struct OpenTerminalsSection: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Section {
            ForEach(model.terminals) { t in
                OpenTerminalRow(terminal: t)
            }
        } header: {
            Text("已打开的终端")
        } footer: {
            Text("返回后终端仍保持接入（最多 \(AppModel.terminalLimit) 个）。断开后，会话仍在电脑上运行。")
        }
    }
}

/// 开着的一个终端：状态点、会话的名字、哪台电脑、接着没有
private struct OpenTerminalRow: View {
    @Environment(AppModel.self) private var model
    let terminal: TerminalSession

    var body: some View {
        let t = terminal
        let close = t.isClosed ? "关闭" : "断开"
        Button { model.show(t) } label: {
            HStack(spacing: 12) {
                StatusDot(kind: t.dot)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(Texts.sessionTitle(t.session)).font(.body.weight(.medium)).foregroundStyle(.primary).lineLimit(1)
                        if t.readOnly { Tag(text: "只看", systemImage: "eye") }
                    }
                    Text(t.peer.name + " · " + (Texts.linkState(t.state) ?? "已接入"))
                        .font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                }
                Spacer(minLength: 6)
                Image(systemName: "chevron.right").font(.caption.weight(.semibold)).foregroundStyle(.tertiary)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .moveDisabled(true)
        .swipeActions(edge: .trailing, allowsFullSwipe: true) {
            Button { model.closeTerminal(t) } label: { Label(close, systemImage: "xmark.circle") }
        }
        .contextMenu {
            Button { model.show(t) } label: { Label("切换到它的终端", systemImage: "arrow.right.circle") }
            if !t.session.info.viewOnly, !t.isClosed {
                Button { model.attach(t.session, on: t.peer, readOnly: !t.readOnly) } label: {
                    Label(t.readOnly ? "改成可以操作（重新接入）" : "改成只看（重新接入）", systemImage: t.readOnly ? "keyboard" : "eye")
                }
            }
            Button { model.closeTerminal(t) } label: { Label(close, systemImage: "xmark.circle") }
        }
        .accessibilityHint("切换到这个终端")
        .accessibilityActions {
            Button(close) { model.closeTerminal(t) }
        }
    }
}
