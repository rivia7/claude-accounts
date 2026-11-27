import AppKit
import SwiftUI

/// 「远程会话」一页（PROTOCOL 9）：这台 Mac 上和配对过的机器上可以接入的会话、配对过的机器；接入一个会话就是在这一页里开一块终端
/// （`cca remote attach` 跑在 App 开的伪终端上，SwiftTerm 画）。有哪些会话、接不接得上、对方让做什么都是 cca 给的，这里不重算。
/// 可以同时接着几个：开着终端时页顶多出一排（`RemoteTabStrip`：会话列表、每个终端各一格），点哪个显示哪个，别的照样接着。
/// 只有 cca 有远程会话（`hello.remote`）时侧栏才列这一页。
struct RemoteSessionsPage: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 0) {
            if !model.remoteTerminals.isEmpty { RemoteTabStrip() }
            if let t = model.remoteTerminal {
                RemoteTerminalPane(terminal: t).id(t.id)
            } else {
                RemoteSessionList()
            }
        }
        .navigationTitle("远程会话")
    }
}

// MARK: 切换

/// 页顶的一排：最左边是「会话列表」，后面是开着的每个终端（按接入的先后）。点一格显示它；终端那几格右边的 × 是断开并关掉。
/// 放不下时横着滚。显示菜单里另有「下一个 / 上一个远程会话」（⇧⌘] / ⇧⌘[，`AppModel.cycleRemote`）。
private struct RemoteTabStrip: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                RemoteTab(selected: model.remoteTerminal == nil, help: "可接入的会话和已配对的机器；已打开的终端保持接入") {
                    model.showRemoteList()
                } label: {
                    Label("会话列表", systemImage: "list.bullet")
                }
                ForEach(model.remoteTerminals) { t in
                    RemoteTab(selected: model.remoteShown == t.id, help: place(t),
                              close: { model.closeRemoteTerminal(t) },
                              closeHelp: t.hasEnded ? "关闭这个终端" : "断开并关闭这个终端；会话会继续运行") {
                        model.showRemoteTerminal(t)
                    } label: {
                        HStack(spacing: 7) {
                            RemoteTerminalDot(status: t.status)
                            Text(Texts.remoteTab(t.session)).truncationMode(.middle).frame(maxWidth: 230)
                            if t.readOnly { Image(systemName: "eye").font(.caption).accessibilityLabel("只看") }
                        }
                    }
                }
            }
            .padding(.horizontal, 20)
        }
        .padding(.top, 12)
        .accessibilityLabel("开着的终端")
    }

    private func place(_ t: RemoteTerminal) -> String {
        let s = t.session
        return [s.isLocal ? "这台 Mac" : nonEmpty(s.machine) ?? "另一台机器", nonEmpty(s.cwd), t.hasEnded ? "已经断开" : nil]
            .compactMap { $0 }.joined(separator: " · ")
    }
}

/// 那一排里的一格：选中的那格底色深一点、字是正文色，别的是灰字。右边可以带一个关掉的 ×（单独的按钮，不套在大的里面）。
private struct RemoteTab<Content: View>: View {
    let selected: Bool
    let help: String
    var close: (() -> Void)?
    var closeHelp = ""
    let action: () -> Void
    @ViewBuilder let label: Content

    var body: some View {
        HStack(spacing: 0) {
            Button(action: action) {
                label
                    .lineLimit(1)
                    .padding(.leading, 11).padding(.trailing, close == nil ? 11 : 5)
                    .frame(height: 28)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(help)
            .accessibilityAddTraits(selected ? .isSelected : [])
            if let close {
                Button(action: close) {
                    Image(systemName: "xmark").font(.system(size: 9, weight: .bold))
                        .frame(width: 20, height: 28)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .padding(.trailing, 4)
                .help(closeHelp)
                .accessibilityLabel(closeHelp)
            }
        }
        .font(.callout.weight(selected ? .semibold : .regular))
        .foregroundStyle(selected ? Color.primary : Color.secondary)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(.quaternary.opacity(selected ? 1 : 0.35)))
        .accessibilityElement(children: .contain)
    }
}

/// 一个终端的状态点：接着是绿的，还没起是淡的，断开、结束了是空心的。
private struct RemoteTerminalDot: View {
    let status: RemoteTerminal.Status

    var body: some View {
        Group {
            switch status {
            case .running: Circle().fill(.green)
            case .waiting: Circle().fill(.quaternary)
            default: Circle().strokeBorder(.secondary, lineWidth: 1.5)
            }
        }
        .frame(width: 8, height: 8)
        .accessibilityHidden(true)
    }
}

// MARK: 列表

private struct RemoteSessionList: View {
    @Environment(AppModel.self) private var model
    /// 要取消配对的那一台（等确认）
    private let unpairing = State<RemotePeer?>(initialValue: nil)

    var body: some View {
        let snap = model.remote
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                ViewThatFits(in: .horizontal) {
                    HStack(alignment: .center) { heading; Spacer(minLength: 20); refresh(snap) }
                    VStack(alignment: .leading, spacing: 14) { heading; refresh(snap) }
                }
                switchCard(snap.doc)
                problems(snap)
                TimelineView(.periodic(from: .now, by: 30)) { context in
                    VStack(alignment: .leading, spacing: 22) {
                        sessions(snap, now: context.date.timeIntervalSince1970)
                        peers(snap, now: context.date.timeIntervalSince1970)
                    }
                }
                if model.remoteLaunchAvailable { RemoteLaunchSection() }
            }
            .padding(.top, 24).padding(.bottom, 36).pageColumn()
        }
        .confirmationDialog(unpairing.wrappedValue.map { "取消和「\($0.name)」的配对？" } ?? "",
                            isPresented: Binding(get: { unpairing.wrappedValue != nil }, set: { if !$0 { unpairing.wrappedValue = nil } }),
                            titleVisibility: .visible) {
            Button("取消配对", role: .destructive) {
                if let p = unpairing.wrappedValue { model.unpairRemote(p) }
                unpairing.wrappedValue = nil
            }
            Button("不取消", role: .cancel) { unpairing.wrappedValue = nil }
        } message: {
            Text("双方都不能再接入对方的会话，正在接入的会断开。之后如需使用，需要重新配对。")
        }
    }

    private var heading: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                Text("远程会话").font(.system(size: 24, weight: .semibold))
                if model.remoteBusy { ProgressView().controlSize(.small) }
            }
            Text("关闭终端后会话仍会运行；可以在这里重新接入，也可以接入另一台已配对机器上的会话。")
                .font(.callout).foregroundStyle(.secondary)
        }
    }

    private func refresh(_ snap: RemoteSnapshot) -> some View {
        HStack(spacing: 10) {
            TimelineView(.periodic(from: .now, by: 30)) { context in
                if model.remoteLoading, snap.fetchedEpoch == nil {
                    ProgressView().controlSize(.small)
                } else if let at = snap.fetchedEpoch {
                    Text(Texts.agoPhrase(context.date.timeIntervalSince1970 - at) + "查询")
                        .font(.caption).foregroundStyle(.secondary).monospacedDigit()
                }
            }
            SectionAction(model.remoteLoading ? "刷新中…" : "刷新", systemImage: "arrow.clockwise") { model.refreshRemote(manual: true) }
                .disabled(model.remoteLoading)
                .help("重新查询这台 Mac 和已配对机器上的会话（会询问 Team 的服务器）")
        }
    }

    /// 开关：之后在终端里启动的会话放不放进会话宿主里运行
    private func switchCard(_ doc: RemoteDoc?) -> some View {
        Card("这台 Mac", systemImage: "desktopcomputer") {
            Toggle(isOn: Binding(get: { doc?.enabled == true }, set: { model.setRemoteEnabled($0) })) {
                VStack(alignment: .leading, spacing: 5) {
                    Text("之后启动的会话可以重新接入")
                    Text("打开后，在终端中用 cca 启动的会话在关闭终端后仍会运行，可以在这里或已配对的机器上重新接入。已打开的会话不受影响。")
                        .font(.callout).foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .toggleStyle(.switch)
            .disabled(doc == nil || model.remoteBusy)
            if let doc, !doc.relayAvailable {
                Divider()
                Label(nonEmpty(doc.relayMessage) ?? "这台 Mac 无法使用中转：只能接入这台 Mac 上的会话，无法与其他机器配对。", systemImage: "info.circle")
                    .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            if model.isFixtureMode {
                Text("演示数据：开关、配对不会真的改，终端里是一屏示意。").font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private func problems(_ snap: RemoteSnapshot) -> some View {
        if let e = snap.error {
            Banner(systemImage: "exclamationmark.triangle", tint: .orange, title: "无法获取可接入的会话", message: e) {
                Button("重试") { model.refreshRemote(manual: true) }
                    .buttonStyle(.borderedProminent).disabled(model.remoteLoading)
            }
        }
        // 这台 Mac 用不了中转（relay_off）已经写在上面的卡片里，不再重复
        ForEach(Array((snap.doc?.errors ?? []).filter { $0.code != .relayOff }.enumerated()), id: \.offset) { _, p in
            Banner(systemImage: "exclamationmark.triangle", tint: .orange, title: "无法获取其他机器上的会话", message: p.message)
        }
    }

    // MARK: 会话

    private struct SessionGroup: Identifiable {
        let id: String
        let title: String
        var sessions: [RemoteSession]
        /// 那台机器允许这台 Mac 远程启动：标题旁边是「新建会话…」（PROTOCOL 9.5）
        var launcher: RemotePeerLauncher? = nil
    }

    /// 这台 Mac 上的在前，别的机器按 cca 给的先后，一台一组；允许这台 Mac 远程启动的机器没有会话也有一组
    private func groups(_ doc: RemoteDoc) -> [SessionGroup] {
        var out: [SessionGroup] = []
        for s in doc.sessions {
            let id = s.isLocal ? "" : (s.peer ?? s.machine ?? "?")
            if let i = out.firstIndex(where: { $0.id == id }) {
                out[i].sessions.append(s)
            } else {
                let name = s.isLocal ? "这台 Mac" : nonEmpty(s.machine) ?? doc.peers.first { $0.id == s.peer }?.name ?? "另一台机器"
                out.append(SessionGroup(id: id, title: name, sessions: [s]))
            }
        }
        for l in model.remoteLaunchMachinesAvailable ? doc.peerLaunchers : [] {
            if let i = out.firstIndex(where: { $0.id == l.peer }) {
                out[i].launcher = l
            } else {
                let name = nonEmpty(l.machine) ?? doc.peers.first { $0.id == l.peer }?.name ?? "另一台机器"
                out.append(SessionGroup(id: l.peer, title: name, sessions: [], launcher: l))
            }
        }
        return out.filter { $0.id.isEmpty } + out.filter { !$0.id.isEmpty }
    }

    /// 「新建会话…」：照那台机器上的模板在那台机器上起一个，起好了在这里接入
    private func newSession(_ l: RemotePeerLauncher) -> some View {
        RowAction("新建会话…", systemImage: "plus.rectangle.on.rectangle") { model.newRemoteSession(on: l) }
            .disabled(!l.online || l.templates.isEmpty)
            .help(!l.online ? "「\(l.machine)」暂时未连接服务器，无法新建会话"
                  : l.templates.isEmpty ? "「\(l.machine)」上还没有可用的模板" : "用「\(l.machine)」上的模板在那台机器上新建会话，启动后在这里接入")
    }

    @ViewBuilder
    private func sessions(_ snap: RemoteSnapshot, now: Double) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader("可以接入的会话") { EmptyView() }
            if let doc = snap.doc {
                let gs = groups(doc)
                if gs.isEmpty {
                    Text(doc.enabled ? "现在没有可以接入的会话。在终端里用 cca 启动一个会话（比如 cca @账号），它就会出现在这里。"
                         : "现在没有可以接入的会话。先打开上面的开关，之后在终端里启动的会话才接得上。")
                        .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                        .padding(.horizontal, 4)
                } else {
                    ForEach(gs) { g in
                        VStack(alignment: .leading, spacing: 9) {
                            HStack(spacing: 8) {
                                Text(g.title).font(.subheadline.weight(.semibold)).foregroundStyle(.secondary)
                                Spacer(minLength: 8)
                                if let l = g.launcher { newSession(l) }
                            }
                            if g.sessions.isEmpty {
                                Text("现在没有开着的会话：「新建会话…」照那台机器上的模板起一个").font(.callout).foregroundStyle(.secondary)
                            }
                            ForEach(g.sessions) { s in
                                RemoteSessionLine(session: s, now: now)
                            }
                        }
                        .padding(.horizontal, 4)
                    }
                }
            } else if !model.remoteLoading, snap.error == nil {
                Text("尚未查询").font(.callout).foregroundStyle(.secondary).padding(.horizontal, 4)
            }
        }
    }

    // MARK: 配对过的机器

    @ViewBuilder
    private func peers(_ snap: RemoteSnapshot, now: Double) -> some View {
        let doc = snap.doc
        let usable = doc?.relayAvailable == true
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader("已配对的机器") {
                PairAction(title: "显示配对码…", systemImage: "number.square", hubs: doc?.relayHubs ?? []) { model.pairRemote(show: true, hub: $0) }
                    .disabled(!usable)
                    .help(usable ? "让另一台机器接入这台 Mac 上的会话：这里显示配对码，在那台机器上输入。每个 Team 的账号的会话走这个 Team 自己的服务器：在哪台上配对，对方看得到的就是走那台的会话"
                          : nonEmpty(doc?.relayMessage) ?? "这台 Mac 无法使用中转")
                SectionAction("输入配对码…", systemImage: "keyboard") { model.pairRemote(show: false) }
                    .disabled(!usable)
                    .help(usable ? "接入另一台机器上的会话：在那台机器上显示配对码（cca remote pair），在这里输入" : nonEmpty(doc?.relayMessage) ?? "这台 Mac 无法使用中转")
                if model.remoteWebAvailable {
                    PairAction(title: "和手机配对…", systemImage: "iphone", hubs: doc?.relayHubs ?? []) { model.pairRemotePhone(hub: $0) }
                        .disabled(!usable)
                        .help(usable ? "在 iPhone 上接入这台 Mac 上的会话：这里会显示二维码，用 iPhone 上的 cca App 扫码即可配对"
                              : nonEmpty(doc?.relayMessage) ?? "这台 Mac 无法使用中转")
                }
            }
            VStack(alignment: .leading, spacing: 9) {
                if let ps = doc?.peers, !ps.isEmpty {
                    ForEach(ps) { p in
                        RemotePeerLine(peer: p, now: now) { unpairing.wrappedValue = p }
                    }
                } else if doc != nil {
                    Text("暂无已配对的机器。").font(.callout).foregroundStyle(.secondary)
                }
                Text("配对是有方向的：显示配对码的那台机器上的会话，可以被输入配对码的那台接入。两台机器经 Team 的服务器中转，内容端到端加密，服务器看不到。")
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            .padding(.horizontal, 4)
        }
    }
}

/// 一个会话：状态点、名字、在哪个文件夹、开了多久；右边是「接入」（那台机器只让看时是「只看」），接不上时说原因。
/// 这台 Mac 上的会话还能「结束…」（等于关掉它的终端；cca 先问一句）。
private struct RemoteSessionLine: View {
    @Environment(AppModel.self) private var model
    let session: RemoteSession
    let now: Double

    var body: some View {
        let detail = Texts.remoteDetail(session, now: now)
        let canEnd = session.isLocal && model.remoteEndAvailable
        // 这个会话在这里已经接着（开着它的终端、还没断开）：右边是「切换」，不再接一次
        let open = model.remoteTerminal(for: session).flatMap { $0.hasEnded ? nil : $0 }
        HStack(spacing: 10) {
            Group {
                switch session.state {
                case .ok: Circle().fill(.green)
                case .busy: Circle().strokeBorder(.orange, lineWidth: 1.5)
                default: Circle().strokeBorder(.secondary, lineWidth: 1.5)
                }
            }
            .frame(width: 8, height: 8)
            .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 8) {
                    Text(nonEmpty(Texts.remoteTitle(session)) ?? session.short).fontWeight(.medium).lineLimit(1)
                    if let team = session.team { Tag(text: model.teams.first { $0.id == team }?.name ?? "Team", color: .secondary) }
                    if session.access == .view { Tag(text: "只看", systemImage: "eye") }
                    if open?.isRunning == true { Tag(text: "已接入", color: .green) }
                }
                if !detail.isEmpty {
                    Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle).monospacedDigit()
                }
            }
            .help(detail)
            Spacer(minLength: 8)
            // 「结束…」放在左边：各行最右边那一列都是「接入」/「只看」，上下对得齐
            if canEnd {
                RowAction("结束…", systemImage: "stop.circle") { model.endRemote(session) }
                    .disabled(model.remoteBusy)
                    .help("结束这个会话：相当于关闭它的终端，其中的程序会退出（会先确认）")
            }
            if let open {
                // 换一种接法（只看 / 操作）是断开原来的再接一次：放在右键里，那台机器只让看的没有这一项
                RowAction("切换", systemImage: "arrow.right.circle") { model.showRemoteTerminal(open) }
                    .help(session.access == .view ? "已接入：切换到它的终端" : "已接入：切换到它的终端（右键可改为\(open.readOnly ? "操作" : "只看")）")
                    .contextMenu {
                        Button("切换到它的终端") { model.showRemoteTerminal(open) }
                        if session.access != .view {
                            Button(open.readOnly ? "改成可以操作（重新接入）" : "改成只看（重新接入）") {
                                model.attachRemote(session, readOnly: !open.readOnly)
                            }
                        }
                    }
            } else if let note = Texts.remoteStateNote(session) {
                Text(note).foregroundStyle(.secondary)
            } else if session.access == .view {
                RowAction("只看", systemImage: "eye") { model.attachRemote(session, readOnly: true) }
                    .help("在这里打开一个终端看这个会话：那台机器只让看，不能操作")
            } else {
                RowAction("接入", systemImage: "terminal") { model.attachRemote(session, readOnly: false) }
                    .help("在这里打开一个终端，查看和操作这个会话（右键可选择只看）")
                    .contextMenu {
                        Button("接入") { model.attachRemote(session, readOnly: false) }
                        Button("只看，不操作") { model.attachRemote(session, readOnly: true) }
                    }
            }
        }
        .font(.callout)
        .frame(minHeight: 34)
        .accessibilityElement(children: .contain)
    }
}

/// 配对过的一台机器：名字、什么时候配的、它能对这台 Mac 的会话做什么（能改）、取消配对。
/// 显示配对码的那两个按钮：只用得了一台中转服务器时就是一个按钮；有好几台（在几个 Team 里）时点开选在哪台上配对——
/// 配对记在哪个 Team 的服务器上，对方经它看得到的就是那个 Team 的账号的会话。
private struct PairAction: View {
    let title: String
    let systemImage: String
    let hubs: [RemoteHub]
    let action: (RemoteHub?) -> Void

    var body: some View {
        if hubs.count > 1 {
            Menu {
                ForEach(hubs) { hub in
                    Button("在\(hub.name)上") { action(hub) }
                }
            } label: {
                Label(title, systemImage: systemImage)
            }
            .glassButton(prominent: false)
            .controlSize(.regular)
            .fixedSize()
        } else {
            SectionAction(title, systemImage: systemImage) { action(nil) }
        }
    }
}

private struct RemotePeerLine: View {
    @Environment(AppModel.self) private var model
    let peer: RemotePeer
    let now: Double
    let unpair: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            line
            if model.remoteLaunchOffered(peer) { launchRow }
        }
    }

    private var line: some View {
        MachineLine(name: nonEmpty(peer.name) ?? peer.short, detail: Texts.remotePeerDetail(peer, now: now)) {
            if peer.web { Tag(text: "手机", color: .secondary) }
            if let team = peer.team { Tag(text: model.teams.first { $0.id == team }?.name ?? "Team", color: .secondary) }
            if peer.state == .gone { Tag(text: "对方已经取消配对", color: .orange) }
        } trailing: {
            Text("它接入这台 Mac 时").font(.caption).foregroundStyle(.secondary)
            Picker("它接入这台 Mac 时", selection: Binding(get: { peer.incoming }, set: { model.setRemoteAccess(peer, $0) })) {
                ForEach([RemoteAccess.control, .view, .none], id: \.self) { a in
                    Text(Texts.remoteAccess(a)).tag(a)
                }
                if !peer.incoming.isKnown { Text(peer.incoming.rawValue).tag(peer.incoming) }
            }
            .labelsHidden()
            .pickerStyle(.menu)
            .fixedSize()
            .disabled(model.remoteBusy)
            .help("查看和操作：它可以接入这台 Mac 上的会话并操作；只看：只能查看；禁止接入：它无法接入（这台 Mac 仍可以接入它的会话）")
            RowAction("取消配对…", systemImage: "xmark.circle", action: unpair)
                .disabled(model.remoteBusy)
                .help("取消和这台机器的配对：两边都不能再接入对方的会话")
        }
    }

    /// 能不能用「远程启动」里的模板在这台 Mac 上新建会话（手机，cca 够新时配对过的电脑也有；打开时 cca 先问一句）。它得能看和操作这台 Mac 的会话：
    /// 不然开关停用、说原因
    private var launchRow: some View {
        let blocked = model.remoteLaunchBlocked(peer)
        return HStack(spacing: 8) {
            Toggle("允许远程启动", isOn: Binding(get: { peer.launch }, set: { model.setRemoteLaunch(peer, $0) }))
                .toggleStyle(.switch)
                .controlSize(.mini)
                .disabled(model.remoteBusy || blocked != nil)
            Text(blocked ?? (peer.launch ? "它能用「远程启动」里的模板在这台 Mac 上新建会话" : "打开后它能用「远程启动」里的模板在这台 Mac 上新建会话"))
                .font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.tail)
        }
        .font(.callout)
        .padding(.leading, 30)   // 和机器的名字对齐（图标 20 + 间距 10）
        .help(blocked ?? "打开以后，它能挑一个模板在这台 Mac 上新建会话，再接入它：账号、文件夹、参数都照模板")
    }
}

// MARK: 远程启动

/// 「远程启动」（PROTOCOL 9.4）：手机用这台 Mac 上的模板新建会话。模板写好用哪个工具、哪个账号、在哪个文件夹、带哪些参数，
/// 手机只能挑一个起（可以带一句开场白）；哪台手机能用在「已配对的机器」里开。只有 cca 有远程启动（`hello.remote_launch`）时才有这一节。
private struct RemoteLaunchSection: View {
    @Environment(AppModel.self) private var model
    /// 要删掉的那个模板（等确认）
    private let deleting = State<RemoteTemplate?>(initialValue: nil)

    var body: some View {
        let snap = model.remoteTemplates
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader("远程启动") {
                if model.remoteTemplatesLoading, snap.doc != nil { ProgressView().controlSize(.small) }
                SectionAction("新建模板…", systemImage: "plus") { model.editRemoteTemplate() }
                    .disabled(model.remoteBusy)
                    .help("设定用哪个工具、哪个账号、在哪个文件夹、带哪些参数启动会话：允许远程启动的已配对设备可以选择一个模板，在这台 Mac 上新建会话")
            }
            VStack(alignment: .leading, spacing: 9) {
                if let running = model.remoteLauncherRunning { launcher(running) }
                if let e = snap.error {
                    Banner(systemImage: "exclamationmark.triangle", tint: .orange, title: "无法获取模板", message: e) {
                        Button("重试") { model.refreshRemoteTemplates() }
                            .buttonStyle(.borderedProminent).disabled(model.remoteTemplatesLoading)
                    }
                }
                if let doc = snap.doc {
                    if doc.templates.isEmpty {
                        Text("还没有模板。").font(.callout).foregroundStyle(.secondary)
                    } else {
                        ForEach(doc.templates) { t in
                            RemoteTemplateLine(template: t) { deleting.wrappedValue = t }
                        }
                    }
                } else if model.remoteTemplatesLoading {
                    ProgressView().controlSize(.small)
                }
                Text(model.remoteLaunchMachinesAvailable
                     ? "在「已配对的机器」中为一台手机或电脑打开「允许远程启动」后，它就能选择一个模板在这台 Mac 上新建会话并接入：账号、文件夹、参数都按模板设置，对方无法修改，只能附带一句开场白。"
                     : "在「已配对的机器」中为一台手机打开「允许远程启动」后，它就能选择一个模板在这台 Mac 上新建会话并接入：账号、文件夹、参数都按模板设置，手机上无法修改，只能附带一句开场白。")
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            .padding(.horizontal, 4)
        }
        .confirmationDialog(deleting.wrappedValue.map { "删除模板「\($0.name)」？" } ?? "",
                            isPresented: Binding(get: { deleting.wrappedValue != nil }, set: { if !$0 { deleting.wrappedValue = nil } }),
                            titleVisibility: .visible) {
            Button("删除", role: .destructive) {
                if let t = deleting.wrappedValue { model.removeRemoteTemplate(t) }
                deleting.wrappedValue = nil
            }
            Button("不删", role: .cancel) { deleting.wrappedValue = nil }
        } message: {
            Text("配对的设备上就看不到它了；已经用它起的会话照常运行。")
        }
    }

    /// 等配对的设备来起会话的后台进程在不在跑（有允许远程启动的手机、电脑时才说）
    private func launcher(_ running: Bool) -> some View {
        Label {
            Text(running ? "远程启动的后台进程在运行：允许远程启动的配对的设备现在就能新建会话"
                 : "远程启动的后台进程未运行，暂时无法新建会话（刚打开时需要等几秒；可在终端运行 cca remote launcher 查看状态）")
                .foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        } icon: {
            Image(systemName: running ? "checkmark.circle" : "exclamationmark.triangle")
                .foregroundStyle(running ? Color.green : Color.orange)
        }
        .font(.callout)
    }
}

/// 一个模板：名字、工具，用不了时标出原因；第二行是账号、文件夹、参数。右边「修改…」「删除…」
private struct RemoteTemplateLine: View {
    @Environment(AppModel.self) private var model
    let template: RemoteTemplate
    let remove: () -> Void

    var body: some View {
        let t = template
        let detail = Texts.remoteTemplateDetail(t, dir: model.displayPath(t.dir))
        HStack(spacing: 10) {
            Image(systemName: "doc.text").foregroundStyle(.secondary).frame(width: 20)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 8) {
                    Text(nonEmpty(t.name) ?? t.id).fontWeight(.medium).lineLimit(1)
                    if let tool = t.tool { Tag(text: Texts.cliProduct(tool)) }
                    if let p = t.problem {
                        Tag(text: Texts.remoteTemplateProblem(p), color: .orange).help(Texts.remoteTemplateProblemHelp(p))
                    }
                }
                if !detail.isEmpty {
                    Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                }
            }
            .help(detail)
            Spacer(minLength: 8)
            RowAction("修改…", systemImage: "pencil") { model.editRemoteTemplate(t) }
                .disabled(model.remoteBusy)
                .help("改这个模板的名字、账号、文件夹、参数")
            RowAction("删除…", systemImage: "trash") { remove() }
                .disabled(model.remoteBusy)
                .help("删除这个模板：已配对的设备将不再看到它")
        }
        .font(.callout)
        .frame(minHeight: 34)
        .accessibilityElement(children: .contain)
    }
}

/// 新建、修改一个模板（弹层）：名字、工具、账号、文件夹、参数。存的时候 cca 校验，不行的原因写在下面，接着改。
struct RemoteTemplateSheet: View {
    @Environment(AppModel.self) private var model
    let draft: RemoteTemplateDraft

    var body: some View {
        let d = draft
        let tools = model.remoteTemplateTools
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 6) {
                Text(d.original == nil ? "新建模板" : "修改模板").font(.title3.weight(.semibold))
                Text("在「已配对的机器」中允许远程启动的设备可以选择这个模板，在这台 Mac 上新建会话：账号、文件夹、参数都按这里的设置，对方无法修改。")
                    .foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            field("名字") {
                TextField("", text: Binding(get: { d.name }, set: { d.name = $0 }), prompt: Text("挑模板时看到的，比如「网站：修 bug」"))
                    .textFieldStyle(.roundedBorder)
            }
            if tools.count > 1 {
                field("工具") {
                    Picker("工具", selection: Binding(get: { d.tool }, set: { model.setRemoteTemplateTool(d, $0) })) {
                        ForEach(tools, id: \.self) { t in Text(Texts.cliProduct(t)).tag(t) }
                    }
                    .labelsHidden()
                    .pickerStyle(.segmented)
                    .fixedSize()
                }
            }
            field("账号", note: accountNote) {
                Picker("账号", selection: Binding(get: { d.account }, set: { d.account = $0 })) {
                    if d.account.isEmpty { Text("选一个账号").tag("") }
                    ForEach(model.accounts(d.tool)) { a in
                        Text(a.isAPI ? a.displayName + "（第三方 API）" : a.displayName).tag(a.key)
                    }
                }
                .labelsHidden()
                .pickerStyle(.menu)
                .fixedSize()
            }
            field("文件夹") {
                HStack(spacing: 10) {
                    Text(d.dir.isEmpty ? "未选择" : model.displayPath(d.dir))
                        .foregroundStyle(d.dir.isEmpty ? Color.secondary : Color.primary)
                        .lineLimit(1).truncationMode(.middle)
                        .help(d.dir)
                    Spacer(minLength: 8)
                    Button("选择…") { model.chooseRemoteTemplateDirectory(d) }
                }
            }
            field("参数", note: "与终端中的写法相同：用空格分隔，含空格的用引号括起来，例如 --model opus --append-system-prompt '说中文'。没有可留空") {
                TextField("", text: Binding(get: { d.args }, set: { d.args = $0 }), axis: .vertical)
                    .textFieldStyle(.roundedBorder)
                    .font(.system(.body, design: .monospaced))
                    .lineLimit(1...4)
            }
            if let e = d.error {
                Label(e, systemImage: "exclamationmark.circle.fill")
                    .font(.callout)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
            HStack {
                if d.saving { ProgressView().controlSize(.small) }
                Spacer()
                Button("取消") { model.dismissSheet() }
                    .keyboardShortcut(.cancelAction)
                Button("保存") { model.saveRemoteTemplate(d) }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.glassProminent)
                    .disabled(!d.canSave)
            }
        }
        .padding(22)
        .frame(width: 520)
    }

    /// 账号下面那一句：这个工具还没有账号；改的模板原来的账号没了
    private var accountNote: String? {
        let d = draft
        if model.accounts(d.tool).isEmpty { return "\(Texts.cliProduct(d.tool)) 还没有账号" }
        if let t = d.original, d.account.isEmpty { return "模板原来的账号（\(nonEmpty(t.accountName) ?? t.account)）已不在账号表中，请换一个" }
        return nil
    }

    private func field<Content: View>(_ label: String, note: String? = nil, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(label).font(.callout.weight(.medium))
            content()
            if let note {
                Text(note).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// 在配对过的另一台机器上新建会话（弹层，PROTOCOL 9.5）：挑那台机器上的一个模板（账号、文件夹、参数都是那台机器写好的，这里改不了），
/// 能带开场白的可以写一句。起好了在这一页里开一个终端接入它；起不成的原因（cca 说的）写在下面，可以再起。
struct RemoteLaunchSheet: View {
    @Environment(AppModel.self) private var model
    let draft: RemoteLaunchDraft

    var body: some View {
        let d = draft
        let l = d.launcher
        let machine = nonEmpty(l.machine) ?? "另一台机器"
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 6) {
                Text("在「\(machine)」上新建会话").font(.title3.weight(.semibold))
                Text("选择那台机器上的一个模板：账号、文件夹、参数都按模板设置（在那台机器上修改），启动后在这里接入。")
                    .foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            Picker("模板", selection: Binding(get: { d.template }, set: { d.template = $0 })) {
                ForEach(l.templates) { t in
                    templateLabel(t).tag(Optional(t.id))
                }
            }
            .labelsHidden()
            .pickerStyle(.radioGroup)
            .disabled(d.starting)
            if l.more > 0 {
                Text("那台机器上还有 \(l.more) 个模板没列出来（说明里放不下）").font(.caption).foregroundStyle(.secondary)
            }
            if let t = d.chosen {
                if t.line {
                    VStack(alignment: .leading, spacing: 5) {
                        Text("开场白").font(.callout.weight(.medium))
                        TextField("", text: Binding(get: { d.line }, set: { d.line = $0 }), prompt: Text("可以不写，比如「先跑一遍测试」"))
                            .textFieldStyle(.roundedBorder)
                            .disabled(d.starting)
                        Text("填写后，会话启动时会作为第一句话发给\(t.tool.map(Texts.cliProduct) ?? "它")。")
                            .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                } else {
                    Text("这个模板不能带开场白（参数里已经有一句话了）。").font(.caption).foregroundStyle(.secondary)
                }
            }
            if !l.online {
                Label("「\(machine)」暂时未连接服务器，无法新建会话", systemImage: "exclamationmark.triangle")
                    .font(.callout).foregroundStyle(.orange).fixedSize(horizontal: false, vertical: true)
            }
            if let e = d.error {
                Label(e, systemImage: "exclamationmark.circle.fill")
                    .font(.callout)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
            HStack(spacing: 10) {
                if d.starting {
                    ProgressView().controlSize(.small)
                    Text("正在「\(machine)」上启动会话…").font(.callout).foregroundStyle(.secondary)
                }
                Spacer()
                Button(d.starting ? "关闭" : "取消") { model.dismissSheet() }
                    .keyboardShortcut(.cancelAction)
                    .help(d.starting ? "会话会继续启动：启动后在「远程会话」页接入，启动失败会弹出提示" : "")
                Button("新建会话") { model.startRemoteLaunch(d) }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.glassProminent)
                    .disabled(!d.canStart)
            }
        }
        .padding(22)
        .frame(width: 520)
    }

    /// 一个模板：名字、工具；第二行是账号、文件夹、参数
    private func templateLabel(_ t: RemotePeerTemplate) -> some View {
        let detail = Texts.remotePeerTemplateDetail(t)
        return VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 8) {
                Text(nonEmpty(t.name) ?? t.id).fontWeight(.medium).lineLimit(1)
                if let tool = t.tool { Tag(text: Texts.cliProduct(tool)) }
            }
            if !detail.isEmpty {
                Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
            }
        }
        .padding(.vertical, 3)
        .help(detail)
    }
}

// MARK: 终端

/// 显示一个终端时的这一页：上面一行是哪个会话、在哪台机器，右边「断开」（断开并关掉这一个，别的照样接着）；下面整块是终端。
/// 会话自己结束、连接断了时终端留着（看得到最后的输出），上面说一句怎么了，可以重新接入或者关掉它。
private struct RemoteTerminalPane: View {
    @Environment(AppModel.self) private var model
    let terminal: RemoteTerminal

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            ended
            TerminalHost(terminal: terminal)
                .padding(8)
                .background(Color(nsColor: TerminalSurface.background))
                .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(.quaternary))
                .accessibilityLabel("终端：" + Texts.remoteTitle(terminal.session))
        }
        .padding(.horizontal, 20).padding(.top, 16).padding(.bottom, 20)
    }

    private var place: String {
        let s = terminal.session
        let team = s.team.map { t in model.teams.first { $0.id == t }.map { "「\($0.name)」" } ?? "Team" }
        return [s.isLocal ? "这台 Mac" : nonEmpty(s.machine) ?? "另一台机器", team, nonEmpty(s.cwd), nonEmpty(terminal.title)]
            .compactMap { $0 }.joined(separator: " · ")
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 12) {
            RemoteTerminalDot(status: terminal.status)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 8) {
                    Text(nonEmpty(Texts.remoteTitle(terminal.session)) ?? terminal.session.short).font(.title3.weight(.semibold)).lineLimit(1)
                    if terminal.readOnly { Tag(text: "只看", systemImage: "eye") }
                }
                Text(place).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
            }
            Spacer(minLength: 12)
            Text(terminal.readOnly ? "选中即复制" : "选中即复制 · 右键粘贴")
                .font(.caption).foregroundStyle(.tertiary).lineLimit(1)
                .help("用鼠标选中的字马上放进剪贴板" + (terminal.readOnly ? "" : "；在终端里点右键粘贴剪贴板里的字")
                      + "。会话里的程序要鼠标时，按住 Option 拖着选")
            if terminal.hasEnded {
                // 还在最近一次查到的列表里、接得上，才给「重新接入」
                if let s = model.remote.doc?.session(terminal.session.id), s.attachable {
                    SectionAction("重新接入", systemImage: "arrow.clockwise") { model.attachRemote(s, readOnly: terminal.readOnly) }
                }
                SectionAction("关闭", systemImage: "xmark.circle") { model.closeRemoteTerminal(terminal) }
                    .help("关闭这个终端（不保留最后的输出）")
            } else {
                SectionAction("断开", systemImage: "xmark.circle") { model.closeRemoteTerminal(terminal) }
                    .help("断开并关闭这个终端；会话会继续运行，其他终端仍保持接入")
            }
        }
        .frame(minHeight: 36)
    }

    @ViewBuilder
    private var ended: some View {
        switch terminal.status {
        case .ended(let code):
            let listed = model.remote.doc?.session(terminal.session.id) != nil
            Label(Texts.remoteEnded(code: code, closedByUser: terminal.closedByUser, listed: listed), systemImage: "info.circle")
                .font(.callout).foregroundStyle(.secondary)
        case .failed(let message):
            Banner(systemImage: "exclamationmark.triangle", tint: .red, title: "无法打开终端", message: message)
        case .waiting, .running:
            EmptyView()
        }
    }
}

/// SwiftTerm 的视图放进 SwiftUI：视图是 RemoteTerminal 自己留着的那一个（TerminalSurface），这里只是把它摆上来。
private struct TerminalHost: NSViewRepresentable {
    @Environment(AppModel.self) private var model
    let terminal: RemoteTerminal

    func makeNSView(context: Context) -> NSView {
        let hooks = model.hooks
        let s = TerminalSurface.of(terminal, openURL: { hooks.openURL($0) }, copy: { hooks.copy($0) })
        // 摆上来就能打字（只看的也要：滚动、选字、复制都要它是第一响应者）
        DispatchQueue.main.async { s.focus() }
        return s.nsView
    }

    func updateNSView(_ view: NSView, context: Context) {}
}

/// 侧栏里「远程会话」那一行：自己的图标（RemoteMark），接着会话时右边说一声。
struct RemoteSessionsRow: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 10) {
            RemoteMark(size: 23).frame(width: 30, height: 30)
            VStack(alignment: .leading, spacing: 2) {
                Text("远程会话").font(.body.weight(.medium))
                Text("接入别处运行的会话").font(.caption).foregroundStyle(.plainSecondary).lineLimit(1)
            }
            Spacer(minLength: 4)
            let n = model.remoteAttachedCount
            if n > 0 {
                Tag(text: n == 1 ? "已接入" : "已接入 \(n) 个", color: .green)
            }
        }
        .sidebarRowPadding()
        .accessibilityElement(children: .combine)
    }
}

/// “远程会话”的图标：终端的提示符（一个尖括号、一道下划线）。和用量统计、客户端更新的图标一套画法（粗圆的线，线宽是边长的 0.15，
/// 不带底）；颜色是 Logo 的紫渐变到玫红（左上角先掺一点白），和别的几个图标分得开。
struct RemoteMark: View {
    var size: CGFloat = 22

    var body: some View {
        Canvas { ctx, s in
            let w = s.width
            func p(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: x * w, y: y * w) }
            var g = Path()
            g.move(to: p(0.16, 0.24)); g.addLine(to: p(0.46, 0.50)); g.addLine(to: p(0.16, 0.76))    // 尖括号
            g.move(to: p(0.56, 0.78)); g.addLine(to: p(0.86, 0.78))                                  // 下划线
            let shade = Gradient(stops: [.init(color: Brand.violet.mix(with: .white, by: 0.25), location: 0),
                                         .init(color: Brand.violet, location: 0.35), .init(color: Brand.rose, location: 1)])
            ctx.stroke(g, with: .linearGradient(shade, startPoint: p(0.15, 0), endPoint: p(0.85, 1)),
                       style: StrokeStyle(lineWidth: w * 0.15, lineCap: .round, lineJoin: .round))
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}
