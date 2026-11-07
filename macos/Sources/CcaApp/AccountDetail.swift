import SwiftUI

/// 一个账号的详情：身份和主要操作（打开 App、网页版）、“现在能不能用、下一步点什么”的一句话、用量、网络出口。
/// 操作放在它作用的那一节旁边：刷新用量在“用量”的标题上，检测出口在“网络出口”的标题上。第一次来时顶上有一张提示卡片。
struct AccountDetail: View {
    @Environment(AppModel.self) private var model
    let ref: AccountRef

    var body: some View {
        if let a = model.account(ref) {
            ScrollView {
                VStack(alignment: .leading, spacing: 32) {
                    TipCard(id: "account-page", title: "这一页怎么看",
                            message: "名字下面那句话告诉你这个账号现在能不能用、下一步点什么。中间是额度，下面是它的网络出口。想看端口、纯净度这些技术细节，点「显示细节」。")
                    DetailHeader(account: a)
                    if !a.isAPI { UsageSection(account: a) }
                    RouteSection(account: a)
                }
                .padding(.horizontal, 28)
                .padding(.top, 24)
                .padding(.bottom, 36)
                .frame(maxWidth: 780, alignment: .leading)
                .frame(maxWidth: .infinity)
            }
            .navigationTitle(a.displayName)
        } else {
            ContentUnavailableView("没有这个账号", systemImage: "person.crop.circle.badge.questionmark",
                                   description: Text("账号表里已经没有它了。"))
        }
    }
}

/// 头部：头像、名字、邮箱和几个标签；右边是打开 App（主按钮）和网页版，没登录时多一个登录。
/// 宽的时候左右排，窄的时候按钮放到下面。正在打开、检测时下面多一行进度（来自 step 事件）。
struct DetailHeader: View {
    @Environment(AppModel.self) private var model
    @Environment(Guide.self) private var guide
    let account: Account

    var body: some View {
        let a = account
        VStack(alignment: .leading, spacing: 14) {
            ViewThatFits(in: .horizontal) {
                HStack(alignment: .center, spacing: 16) {
                    identity(a)
                    Spacer(minLength: 12)
                    actions(a)
                }
                VStack(alignment: .leading, spacing: 16) {
                    identity(a)
                    actions(a)
                }
            }
            if let busy = model.activity(a.ref) {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(progressText(busy)).font(.callout).foregroundStyle(.secondary)
                }
                .accessibilityElement(children: .combine)
            } else {
                // 给新手的一句话：现在能不能用、下一步点哪个按钮（和时间有关，每 30 秒重算）
                TimelineView(.periodic(from: .now, by: 30)) { context in
                    AdviceLine(advice: Advice.make(a, usage: model.usage(for: a), live: model.liveSlot(for: a.ref),
                                                   appLabel: model.appLabel(a.tool), now: context.date.timeIntervalSince1970))
                }
            }
            let reasons = disabledReasons(a)
            if !reasons.isEmpty {
                Label(reasons.joined(separator: "；"), systemImage: "info.circle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func identity(_ a: Account) -> some View {
        HStack(alignment: .center, spacing: 16) {
            BadgeAvatar(badge: a.badge, isAPI: a.isAPI, size: 56)
                .overlay(alignment: .bottomTrailing) {
                    if a.appRunning {
                        RunningDot(size: 15)
                            .offset(x: 1, y: 1)
                            .help("\(model.appLabel(a.tool)) 开着")
                    }
                }
            VStack(alignment: .leading, spacing: 5) {
                HStack(alignment: .center, spacing: 6) {
                    Text(a.displayName)
                        .font(.system(size: 24, weight: .semibold))
                        .lineLimit(1)
                        .textSelection(.enabled)
                    Button {
                        model.beginRename(a.ref)
                    } label: {
                        Image(systemName: "pencil")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(.secondary)
                    }
                    .buttonStyle(.borderless)
                    .disabled(model.activity(a.ref) != nil)
                    .help("改别名（登录、资料和端口都不变）")
                    .accessibilityLabel("改别名")
                }
                if let sub = secondaryIdentity(a) {
                    Text(sub)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .textSelection(.enabled)
                }
                HStack(spacing: 6) { tags(a) }
                    .padding(.top, 2)
            }
        }
    }

    /// 名字下面那行开头：有别名时是邮箱（名字已经是别名了）。
    private func secondaryIdentity(_ a: Account) -> String? {
        guard !a.isAPI, let email = nonEmpty(a.email), email != a.displayName else { return nil }
        return email
    }

    /// 标签：套餐；登录有问题时才说登录状态；App 开着；上次在命令行里用的。
    @ViewBuilder
    private func tags(_ a: Account) -> some View {
        if a.isAPI {
            Tag(text: "第三方 API", color: .blue, systemImage: "key.horizontal")
            if let m = nonEmpty(a.api?.model) { Tag(text: m) }
        } else {
            if let p = nonEmpty(a.plan) { Tag(text: p) }
            if let c = a.loginIssueColor {
                Tag(text: Texts.login(a.login, loginEmail: a.loginEmail), color: c)
            }
        }
        if a.appRunning { Tag(text: "\(model.appLabel(a.tool)) 开着", color: .green) }
        if a.isLast, guide.showDetails { Tag(text: "上次在命令行里用的", systemImage: "terminal") }
    }

    /// 打开 App（主按钮）、网页版；没登录或登错号时，“登录…”是主按钮。第三方 API 账号没有这两个按钮。
    @ViewBuilder
    private func actions(_ a: Account) -> some View {
        let busy = model.activity(a.ref) != nil
        let label = model.appLabel(a.tool)
        let needsLogin = a.login == .loggedOut || a.login == .emailMismatch
        if needsLogin || !a.isAPI {
            GlassRow(spacing: 8) {
                if needsLogin {
                    Button {
                        model.runInTerminal(model.loginHandoff(a))
                    } label: {
                        Label(a.login == .emailMismatch ? "重新登录…" : "登录…", systemImage: "person.badge.key")
                    }
                    .buttonStyle(.glassProminent)
                    .help("在终端里运行 \(model.loginHandoff(a).display)")
                }
                if !a.isAPI {
                    Button {
                        model.openApp(a.ref)
                    } label: {
                        Label("打开 \(label)", systemImage: "macwindow")
                    }
                    .glassButton(prominent: !needsLogin)
                    .disabled(!a.can.app || busy)
                    .help(reason("app", a) ?? "用这个账号的出口和独立的资料目录打开 \(label)")
                    Button {
                        model.openWeb(a.ref)
                    } label: {
                        Label("网页版", systemImage: "safari")
                    }
                    .buttonStyle(.glass)
                    .disabled(!a.can.web || busy)
                    .help(reason("web", a) ?? "在这个账号自己的 Chrome 资料里打开网页版，走账号的出口")
                }
            }
            .controlSize(.large)
        }
    }

    /// 按钮为什么不能用（cca 给的原因码）。API 账号没有这两个按钮，不说。
    private func disabledReasons(_ a: Account) -> [String] {
        if a.isAPI { return [] }
        var out: [String] = []
        let label = model.appLabel(a.tool)
        if !a.can.app, let w = a.canWhy["app"] { out.append("打开 App：" + Texts.canWhy(w, appLabel: label, tool: a.tool)) }
        if !a.can.web, let w = a.canWhy["web"] { out.append("网页版：" + Texts.canWhy(w, appLabel: label, tool: a.tool)) }
        return out
    }

    private func reason(_ action: String, _ a: Account) -> String? {
        guard let w = a.canWhy[action] else { return nil }
        let allowed: Bool
        switch action {
        case "app": allowed = a.can.app
        case "web": allowed = a.can.web
        default: allowed = a.can.usage
        }
        return allowed ? nil : Texts.canWhy(w, appLabel: model.appLabel(a.tool), tool: a.tool)
    }

    private func progressText(_ act: Activity) -> String {
        let appLabel = model.appLabel(account.tool)
        if let s = act.step { return Texts.step(s, appLabel: appLabel) }
        switch act.kind {
        case .app: return "打开 \(appLabel)…"
        case .web: return "打开网页版…"
        case .check: return "检测出口…"
        case .sync: return "同步路由器…"
        case .rename: return "改名…"
        }
    }
}

extension View {
    /// 主按钮用 .glassProminent，其它 .glass。
    @ViewBuilder
    func glassButton(prominent: Bool) -> some View {
        if prominent {
            self.buttonStyle(.glassProminent)
        } else {
            self.buttonStyle(.glass)
        }
    }
}
