import CcaRemoteKit
import SwiftUI

/// 最外面一层：没配对过时是欢迎页；配对过是底下五页，和 Mac App 侧栏的先后、叫法一样：总览（各台电脑上的账号，一个一张卡片）、
/// 用量统计、折合金额、远程会话（可以接入的会话）、共享与同步（电脑所在的 Team）。前三页点一个账号推进它的那一页；
/// 终端推进当时那一页的导航里（看着终端时底下那一排收起来；返回时终端照样接着），配对、设置、新建会话是弹出的一页。
/// 第一次进来（配对好、进了演示）弹新手教程，更新以后弹更新了什么（Guide）；底下浮着的提示（Toast）在标签栏上面；演示模式时每一页的
/// 标题下面写着「演示模式」（demoSubtitle）
struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        Group {
            if model.peers.isEmpty {
                NavigationStack {
                    WelcomeView()
                }
            } else {
                TabView(selection: $model.tab) {
                    Tab("总览", systemImage: "square.grid.2x2", value: AppTab.overview) {
                        NavigationStack(path: $model.accountPath) {
                            AccountsView()
                                .navigationDestination(for: AccountRef.self) { AccountDetailView(ref: $0).demoSubtitle() }
                                .modifier(TerminalDestination(tab: .overview))
                                .demoSubtitle()
                        }
                    }
                    // 角标照 Mac App 侧栏那几行右边的标签：总览「N 个要处理」、远程会话「已接入 N 个」、共享与同步「Team 要处理」
                    // （这里是几台电脑）。标签栏的角标只有数字、只有一种颜色（Mac 上是橙的、绿的）
                    .badge(model.accountsNeedingAttention)
                    Tab("用量统计", systemImage: "chart.bar", value: AppTab.stats) {
                        NavigationStack(path: $model.statsPath) {
                            UsageStatsView()
                                .navigationDestination(for: AccountRef.self) { AccountDetailView(ref: $0).demoSubtitle() }
                                .modifier(TerminalDestination(tab: .stats))
                                .demoSubtitle()
                        }
                    }
                    Tab("折合金额", systemImage: "dollarsign.circle", value: AppTab.cost) {
                        NavigationStack(path: $model.costPath) {
                            CostStatsView()
                                .navigationDestination(for: AccountRef.self) { AccountDetailView(ref: $0, focus: .cost).demoSubtitle() }
                                .modifier(TerminalDestination(tab: .cost))
                                .demoSubtitle()
                        }
                    }
                    Tab("远程会话", systemImage: "terminal", value: AppTab.sessions) {
                        NavigationStack {
                            SessionsView()
                                .modifier(TerminalDestination(tab: .sessions))
                                .demoSubtitle()
                        }
                    }
                    .badge(model.attachedCount)
                    Tab("共享与同步", systemImage: "person.3", value: AppTab.sharing) {
                        NavigationStack {
                            SharingView()
                                .modifier(TerminalDestination(tab: .sharing))
                                .demoSubtitle()
                        }
                    }
                    .badge(model.teamsNeedingAttention)
                }
            }
        }
        .onAppear { model.updatePolling() }
        .sheet(isPresented: Binding(get: { model.pairing != nil }, set: { if !$0 { model.cancelPairing() } })) {
            PairSheet()
        }
        .sheet(isPresented: $model.showSettings) {
            SettingsView()
        }
        .sheet(item: $model.launchFor) { t in
            LaunchSheet(peer: t.peer, preferred: t.account)
        }
        // 新手教程、更新了什么：设置开着时从设置那一页上弹（SettingsView），这里不弹
        .sheet(item: Binding(get: { model.showSettings ? nil : model.guide.sheet }, set: { if $0 == nil { model.guide.dismissSheet() } })) { _ in
            GuideSheet()
        }
        // 底下浮着的提示：在标签栏上面（欢迎页没有标签栏）
        .toastOverlay(bottom: model.peers.isEmpty ? 16 : 64)
        // 第一次进来是新手教程，更新以后是更新了什么：等底下那几页出来、上面别的弹层都关了、没在看终端，再等一会儿（弹层收起来的动画）
        .task(id: guideReady) {
            guard guideReady else { return }
            try? await Task.sleep(for: .milliseconds(700))
            guard !Task.isCancelled, guideReady else { return }
            model.guide.startIfNeeded()
        }
    }

    /// 能自己弹新手教程、更新了什么了：底下那几页出来了，没有配对、设置、新建会话那几页，没在看终端（点了通知进来的先看会话）
    private var guideReady: Bool {
        !model.peers.isEmpty && model.pairing == nil && !model.showSettings && model.launchFor == nil && model.terminal == nil
    }
}

/// 显示着的终端推在哪一页的导航里：从哪一页接入的就在哪一页（返回时回到那里，终端照样接着）。换到另一个开着的终端时整页换掉
/// （id：终端的视图跟着各自的 TerminalSession）
private struct TerminalDestination: ViewModifier {
    @Environment(AppModel.self) private var model
    let tab: AppTab

    func body(content: Content) -> some View {
        content.navigationDestination(item: Binding(get: { model.terminalTab == tab ? model.terminal : nil },
                                                     set: { if $0 == nil, model.terminalTab == tab { model.hideTerminal() } })) { t in
            TerminalScreen(session: t).id(t.id)
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
                    Text("在手机上继续使用电脑上的会话").font(.title2.weight(.semibold)).multilineTextAlignment(.center)
                    Text("在这里查看和操作电脑上用 cca 启动的会话（Claude Code、Codex、Antigravity）。内容端到端加密，中转服务器无法查看。")
                        .font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Card("先和电脑配对", systemImage: "qrcode.viewfinder") {
                    VStack(alignment: .leading, spacing: 8) {
                        step(1, "在 Mac 上打开 cca App 的「远程会话」，点「和手机配对…」（或在终端运行 cca remote pair --web）")
                        step(2, "点下方的「扫码配对」，扫描电脑上的二维码")
                        step(3, "电脑上的会话就会显示在这里")
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
