import CcaRemoteKit
import SwiftUI

/// 一个会话的终端：整页黑底的终端，顶上是哪个会话、在哪台电脑；断了、结束了在终端上面说一句，可以重新接入。
/// 返回就是断开（会话在电脑上照常运行）
struct TerminalScreen: View {
    @Environment(AppModel.self) private var model
    let session: TerminalSession

    var body: some View {
        VStack(spacing: 0) {
            status
            TerminalHost(session: session)
                .background(Color(uiColor: TerminalSurface.background))
                .accessibilityLabel("终端：" + Texts.sessionTitle(session.session))
        }
        .background(Color(uiColor: TerminalSurface.background).ignoresSafeArea(edges: .bottom))
        .navigationBarTitleDisplayMode(.inline)
        // 整页是黑底的终端：这一页的导航栏也是深色（App 是浅色时也一样，和 Mac App 里终端固定深色外观是一个道理）
        .toolbarColorScheme(.dark, for: .navigationBar)
        .toolbarBackground(Color(uiColor: TerminalSurface.background), for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .toolbar {
            ToolbarItem(placement: .principal) {
                VStack(spacing: 1) {
                    HStack(spacing: 6) {
                        StatusDot(kind: dot)
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
        }
        .onDisappear {
            // 返回（不是推到别的页）：断开
            if model.terminal == nil || model.terminal?.id == session.id { session.close() }
        }
    }

    private var dot: StatusDot.Kind {
        switch session.state {
        case .open: return .on
        case .connecting: return .pending
        case .retrying: return .busy
        case .closed: return .off
        }
    }

    private var place: String {
        [session.peer.name, Texts.nonEmpty(session.session.info.cwd), Texts.nonEmpty(session.title)].compactMap { $0 }.joined(separator: " · ")
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

    private var exitText: String? {
        guard case .closed("ended") = session.state, let c = session.exitCode, c != 0 else { return nil }
        return "会话已经结束（退出码 \(c)）"
    }

    /// 能再接一次的：连接上的问题（断了、没接上），不是会话没了、配对没了
    private var canReconnect: Bool {
        guard case .closed(let why) = session.state else { return false }
        return !["ended", "gone", "unpaired", "refused", "version", "auth"].contains(why)
    }
}
