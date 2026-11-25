import CcaRemoteKit
import SwiftUI

/// 最外面一层：没配对过时是欢迎页，配对过是会话列表；终端推进导航里，配对、设置是弹出的一页
struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            Group {
                if model.peers.isEmpty {
                    WelcomeView()
                } else {
                    SessionsView()
                }
            }
            .navigationDestination(item: $model.terminal) { t in
                TerminalScreen(session: t)
            }
        }
        .sheet(isPresented: Binding(get: { model.pairing != nil }, set: { if !$0 { model.cancelPairing() } })) {
            PairSheet()
        }
        .sheet(isPresented: $model.showSettings) {
            SettingsView()
        }
    }
}

/// 还没配对过：Logo、一句话说这是做什么的、扫码配对；也可以先看看演示
struct WelcomeView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView {
            VStack(spacing: 28) {
                Spacer(minLength: 40)
                BrandMark(size: 76)
                VStack(spacing: 10) {
                    Text("在手机上接着用电脑上的会话").font(.title2.weight(.semibold)).multilineTextAlignment(.center)
                    Text("电脑上用 cca 启动的 Claude Code、Codex 会话，在这里看和操作。内容端到端加密，中转的服务器看不到。")
                        .font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Card("先和电脑配对", systemImage: "qrcode.viewfinder") {
                    VStack(alignment: .leading, spacing: 8) {
                        step(1, "在 Mac 上打开 cca App 的「远程会话」，点「和手机配对…」（终端里是 cca remote pair --web）")
                        step(2, "点下面的「扫码配对」，扫电脑上的二维码")
                        step(3, "电脑上那台机器的会话就会出现在这里")
                    }
                    Button {
                        model.startPairing()
                    } label: {
                        Label("扫码配对", systemImage: "qrcode.viewfinder").frame(maxWidth: .infinity)
                    }
                    .glassButton(prominent: true)
                    .controlSize(.large)
                    .padding(.top, 4)
                }
                if model.isDemo {
                    Text("演示模式").font(.footnote).foregroundStyle(.secondary)
                } else {
                    Button("没有电脑？先看看演示") { model.enterDemo() }
                        .font(.callout)
                }
            }
            .padding(.horizontal, 20)
            .padding(.bottom, 30)
            .pageColumn()
        }
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { model.showSettings = true } label: { Image(systemName: "gearshape") }
                    .accessibilityLabel("设置")
            }
        }
    }

    private func step(_ n: Int, _ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text("\(n)").font(.callout.weight(.semibold).monospacedDigit()).foregroundStyle(.secondary).frame(width: 16)
            Text(text).font(.callout).fixedSize(horizontal: false, vertical: true)
        }
    }
}
