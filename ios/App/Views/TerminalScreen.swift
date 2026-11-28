import CcaRemoteKit
import SwiftUI

/// 一个会话的终端：整页黑底的终端，顶上是哪个会话、在哪台电脑（哪个 Team）；断了、结束了在终端上面说一句，可以重新接入。
/// 返回时终端照样接着（会话列表里的「已打开的终端」，和 Mac App 页顶那一排一样）；右上角的菜单里换到别的开着的终端、换一种接法
/// （只看 / 操作：重新接入）、断开（会话在电脑上照常运行）。远程启动的会话另有「结束会话」（先问一句）
struct TerminalScreen: View {
    @Environment(AppModel.self) private var model
    let session: TerminalSession
    @State private var confirmingEnd = false

    var body: some View {
        VStack(spacing: 0) {
            status
            TerminalHost(session: session)
                .background(Color(uiColor: TerminalSurface.background))
                .accessibilityLabel("终端：" + Texts.sessionTitle(session.session))
        }
        .background(Color(uiColor: TerminalSurface.background).ignoresSafeArea(edges: .bottom))
        .navigationBarTitleDisplayMode(.inline)
        // 看着终端时底下那一排（几页）收起来：返回到列表再换页
        .toolbar(.hidden, for: .tabBar)
        // 整页是黑底的终端：这一页的导航栏也是深色（App 是浅色时也一样，和 Mac App 里终端固定深色外观是一个道理）
        .toolbarColorScheme(.dark, for: .navigationBar)
        .toolbarBackground(Color(uiColor: TerminalSurface.background), for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .toolbar {
            ToolbarItem(placement: .principal) {
                VStack(spacing: 1) {
                    HStack(spacing: 6) {
                        StatusDot(kind: session.dot)
                        Text(Texts.sessionTitle(session.session)).font(.subheadline.weight(.semibold)).lineLimit(1)
                        if session.readOnly { Image(systemName: "eye").font(.caption).accessibilityLabel("只看") }
                    }
                    Text(place).font(.caption2).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                }
                .accessibilityElement(children: .combine)
            }
            ToolbarItem(placement: .topBarTrailing) {
                if !session.readOnly, session.isOpen {
                    Button { session.hideKeyboard() } label: { Image(systemName: "keyboard.chevron.compact.down") }
                        .accessibilityLabel("收起键盘")
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                if session.canEnd, session.isOpen, !session.ending {
                    Button { confirmingEnd = true } label: { Image(systemName: "stop.circle") }
                        .accessibilityLabel("结束会话")
                        .accessibilityHint("关掉电脑上的这个会话")
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                menu
            }
        }
        .confirmationDialog("结束这个会话？", isPresented: $confirmingEnd, titleVisibility: .visible) {
            Button("结束会话", role: .destructive) { session.end() }
            Button("取消", role: .cancel) {}
        } message: {
            Text(Texts.endSessionWarning(session.session))
        }
    }

    /// 右上角的菜单：换到别的开着的终端、换一种接法（那台电脑只让看的没有这一项）、断开（结束了、断了的是「关闭」）
    private var menu: some View {
        Menu {
            let others = model.terminals.filter { $0 !== session }
            if !others.isEmpty {
                Section("切换到") {
                    ForEach(others) { t in
                        Button { model.show(t) } label: {
                            Label(Texts.terminalTitle(t.session, on: t.peer), systemImage: t.readOnly ? "eye" : "terminal")
                        }
                    }
                }
            }
            Section {
                if !session.session.info.viewOnly, !session.isClosed {
                    Button { model.attach(session.session, on: session.peer, readOnly: !session.readOnly) } label: {
                        Label(session.readOnly ? "改成可以操作（重新接入）" : "改成只看（重新接入）", systemImage: session.readOnly ? "keyboard" : "eye")
                    }
                }
                Button { model.closeTerminal(session) } label: {
                    if session.isClosed {
                        Label("关闭", systemImage: "xmark.circle")
                    } else {
                        Label { Text("断开"); Text("会话仍在电脑上运行") } icon: { Image(systemName: "xmark.circle") }
                    }
                }
            }
        } label: {
            Image(systemName: "ellipsis.circle")
        }
        .accessibilityLabel("终端选项")
        .accessibilityHint("切换到其他终端、改成只看或可以操作、断开")
    }

    /// 哪台电脑、哪个 Team、哪个文件夹、终端的标题（和 Mac App 终端上面那一行一样）
    /// 标题下面那一行：哪台电脑、哪个 Team、文件夹、会话的标题；演示模式时最前面写着「演示模式」（这一页的标题是自己画的，demoSubtitle 不管用）
    private var place: String {
        [model.isDemo ? "演示模式" : nil, session.peer.name, session.peer.team.map { "「\($0)」" }, Texts.nonEmpty(session.session.info.cwd),
         Texts.nonEmpty(session.title)]
            .compactMap { $0 }.joined(separator: " · ")
    }

    @ViewBuilder
    private var status: some View {
        if let text = Texts.linkState(session.state) {
            HStack(spacing: 10) {
                if session.state == .connecting || session.state == .retrying {
                    ProgressView().controlSize(.small).tint(.white)
                } else {
                    Image(systemName: "info.circle")
                }
                Text(exitText ?? text).font(.footnote).fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 8)
                if session.isClosed, canReconnect {
                    Button("重新接入") {
                        let s = session.terminalSize
                        session.reconnect(cols: s.cols, rows: s.rows)
                    }
                    .buttonStyle(.glass)
                    .controlSize(.small)
                }
            }
            .foregroundStyle(.white.opacity(0.85))
            .padding(.horizontal, 14)
            .padding(.vertical, 9)
            .background(.white.opacity(0.08))
            .accessibilityElement(children: .combine)
        }
    }

    /// 会话自己退出、退出码不是 0 的说退出码；这台手机让它结束的（退出码是被 SIGHUP 结束的 129）不说
    private var exitText: String? {
        guard case .closed("ended") = session.state, !session.ending, let c = session.exitCode, c != 0 else { return nil }
        return "会话已结束（退出码 \(c)）"
    }

    /// 能再接一次的：连接上的问题（断了、没接上），不是会话没了、配对没了
    private var canReconnect: Bool {
        guard case .closed(let why) = session.state else { return false }
        return !["ended", "gone", "unpaired", "refused", "version", "auth"].contains(why)
    }
}

extension TerminalSession {
    /// 状态点：接着是绿的；正在接是淡的；断了、在重新连接是橙圈；关了是灰圈
    var dot: StatusDot.Kind {
        switch state {
        case .open: return .on
        case .connecting: return .pending
        case .retrying: return .busy
        case .closed: return .off
        }
    }
}
