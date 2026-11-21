import SwiftUI

/// 共享与同步：维护者共享给你的账号（维护者的映射，默认关着）的开关，和自己的同步服务器（PROTOCOL 4.13）。状态、账号数、设备都照 cca 给的
/// （`config.policy`、`config.hubs`、`hub devices --json`），不在界面里重算；打开共享、加入、部署是提问会话，关掉共享、移除设备、退出由 cca 先问同意。
struct SharingPage: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                heading
                VStack(alignment: .leading, spacing: 10) {
                    policyCard
                    Text("共享关着时，这台 Mac 只用你自己的账号：不生成给维护者的公钥、不拉维护者的映射、不往维护者的服务器上报。远程会话经维护者的服务器中转，要先打开共享。")
                        .font(.caption).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if model.hubAvailable { HubCard() }
            }
            .padding(.top, 24).padding(.bottom, 36).pageColumn()
        }
        .navigationTitle("共享与同步")
    }

    private var policy: PolicyConfig? { model.state?.config?.policy }

    private var heading: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                Text("共享与同步").font(.system(size: 24, weight: .semibold))
                if model.sharingBusy || model.hubBusy { ProgressView().controlSize(.small) }
            }
            Text(model.hubAvailable ? "维护者共享给你的账号；你自己的账号经你自己的服务器，在几台设备之间同步设备统计和折合金额。"
                 : "维护者共享给你的账号：打开后映射里的账号自动加进来，出口节点由维护者固定。")
                .font(.callout).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var policyCard: some View {
        Card("维护者共享的账号", systemImage: "person.2") {
            Toggle(isOn: Binding(get: { policy?.isOn == true }, set: { model.setSharing($0) })) {
                VStack(alignment: .leading, spacing: 5) {
                    Text("使用维护者共享给你的账号")
                    Text(Self.summary(policy))
                        .font(.callout).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .toggleStyle(.switch)
            .disabled(policy == nil || model.sharingBusy || model.flow != nil)
            if let policy, !Self.rows(policy).isEmpty {
                Divider()
                ForEach(Self.rows(policy), id: \.text) { row in
                    InfoRow(Text(row.text), systemImage: row.symbol, tint: row.tint) { EmptyView() }
                }
            }
        }
    }

    /// 开关下面的那句话：照 cca 给的状态说
    static func summary(_ p: PolicyConfig?) -> String {
        guard let p else { return "正在读取…" }
        switch p.state {
        case .off:
            return "关着：账号表里只有你自己的账号。有维护者共享给你的账号时打开它：把这台 Mac 的一把只读公钥发给维护者，加好后映射里的账号自动加进来。"
        case .on:
            return "开着：映射里的账号自动加进账号表，出口节点由维护者固定。关掉时这些账号移出账号表，登录都留着。"
        default:
            return "开着，还没拉到映射：维护者加上这台 Mac 的公钥后，App 会自动拉取。"
        }
    }

    struct Row { var text: String; var symbol: String; var tint: Color? }

    /// 开关下面的几行明细：映射仓库、账号表里共享的账号、关掉时收起来的账号
    static func rows(_ p: PolicyConfig) -> [Row] {
        var out: [Row] = []
        if p.isOn, let repo = nonEmpty(p.repo) {
            out.append(Row(text: "映射仓库：\(repo)", symbol: "shippingbox", tint: nil))
        }
        if p.state == .on {
            out.append(Row(text: p.accounts > 0 ? "账号表里有 \(p.accounts) 个共享的账号" : "账号表里还没有共享的账号",
                           symbol: "person.crop.circle", tint: nil))
        }
        if p.state == .pending {
            out.append(Row(text: "在等维护者加上这台 Mac 的公钥", symbol: "hourglass", tint: .orange))
        }
        if !p.isOn, p.parked > 0 {
            out.append(Row(text: "关掉时移出了 \(p.parked) 个共享的账号：打开后原样加回来，不用重新登录", symbol: "tray.full", tint: nil))
        }
        return out
    }
}

/// 「自己的同步服务器」（PROTOCOL 4.13）。没加入：部署、加入两个入口。加入了：这台服务器同步得怎么样、加入了的设备（别的能移除）、
/// 复制加入链接、更新服务器，「更多」里是从头部署、退出。移除、退出、从头部署都由 cca 先问同意（对话框里的话是 cca 给的）
private struct HubCard: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Card("自己的同步服务器", systemImage: "server.rack") {
            if let own = model.ownHub {
                joined(own)
            } else {
                notJoined
            }
        }
    }

    // MARK: 没加入

    private var notJoined: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("在几台设备上用自己的账号时，部署一个自己的同步服务器：维护者共享以外的官方账号，设备统计、记下的 API key、折合金额经它和你的其他设备同步。服务器上只有密文，看不到邮箱、登录身份、key 和金额。")
                .font(.callout).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            GlassRow {
                SectionAction("部署到我的服务器…", systemImage: "server.rack", prominent: true) { model.deployHub() }
                    .help("经 SSH 装到你自己的 Linux 服务器上（root 登录；有 Docker 用 Docker，没有用 systemd + python3），这台 Mac 跟着加入")
                SectionAction("粘贴加入链接…", systemImage: "link") { model.joinHub() }
                    .help("已经部署好了：在加入了的设备上复制加入链接，贴到这里")
            }
            .disabled(model.flow != nil)
            Text("已经在别的设备上部署过的，在那台设备上复制加入链接，在这里粘贴。也可以在服务器上运行发布页上的 cca-hub-install.sh（用 root），再粘贴它最后打出的加入链接。")
                .font(.caption).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    // MARK: 加入了

    private func joined(_ own: HubEntry) -> some View {
        TimelineView(.periodic(from: .now, by: 30)) { context in
            let now = context.date.timeIntervalSince1970
            VStack(alignment: .leading, spacing: 12) {
                VStack(alignment: .leading, spacing: 6) {
                    Text(own.host).font(.body.weight(.medium)).textSelection(.enabled)
                    InfoRow(Text(Texts.hubState(own, now: now)), systemImage: Self.symbol(own.state), tint: Self.tint(own.state)) { EmptyView() }
                    InfoRow(Text(Texts.hubSummary(own, now: now)), systemImage: "person.crop.circle") { EmptyView() }
                }
                Divider()
                devices(now: now)
                Divider()
                actions(own)
            }
        }
    }

    static func symbol(_ s: HubState) -> String {
        switch s {
        case .ok: return "checkmark.circle"
        case .pending: return "clock"
        case .off: return "pause.circle"
        default: return "exclamationmark.triangle"
        }
    }

    static func tint(_ s: HubState) -> Color? {
        switch s {
        case .ok: return .green
        case .pending, .off: return nil
        default: return .orange
        }
    }

    /// 标题右边是多久以前问的（没问到时下面的列表是那时候的）和「刷新」，和远程会话页、账号的「设备」一样
    @ViewBuilder
    private func devices(now: Double) -> some View {
        let snap = model.ownHubSnapshot
        let list = model.hubDevices
        VStack(alignment: .leading, spacing: 9) {
            HStack(spacing: 10) {
                Text(list.map { "加入了的设备（\($0.count) 台）" } ?? "加入了的设备").font(.headline)
                Spacer(minLength: 8)
                if model.hubLoading, snap?.fetchedEpoch == nil {
                    ProgressView().controlSize(.small)
                } else if let at = snap?.fetchedEpoch {
                    Text(Texts.agoPhrase(now - at) + "查询").font(.caption).foregroundStyle(.secondary).monospacedDigit()
                }
                SectionAction(model.hubLoading ? "刷新中…" : "刷新", systemImage: "arrow.clockwise") { model.refreshHubDevices(manual: true) }
                    .disabled(model.hubLoading)
                    .help("问一次服务器上有哪些设备")
            }
            if let e = model.hubError {
                Label(e, systemImage: "exclamationmark.triangle")
                    .font(.callout).foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let list {
                ForEach(list) { d in HubDeviceLine(device: d, now: now) }
            } else if !model.hubLoading, model.hubError == nil {
                Text("还没问过服务器").font(.callout).foregroundStyle(.secondary)
            }
            Text("移除的设备现在的密钥作废；它要是还留着加入链接，照样能重新加入。设备丢了、被人拿走了的话，先移除它，再「从头部署」换一个加入链接。")
                .font(.caption).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func actions(_ own: HubEntry) -> some View {
        HStack(spacing: 10) {
            GlassRow {
                SectionAction("复制加入链接", systemImage: "doc.on.doc") { model.copyHubInvite() }
                    .disabled(!own.canInvite || model.hubBusy)
                    .help("给自己的别的设备加入用：在那台设备上「粘贴加入链接…」。谁拿到它都能加入：只发给自己的设备")
                SectionAction("更新服务器…", systemImage: "arrow.triangle.2.circlepath") { model.deployHub() }
                    .help("经 SSH 把服务器上的程序换成这一版 cca 带的：加入链接、数据都不变")
            }
            Spacer(minLength: 8)
            Menu {
                Button("从头部署（换一个加入链接）…") { model.deployHub(reset: true) }
                Divider()
                Button("退出自己的同步服务器…", role: .destructive) { model.leaveHub() }
            } label: {
                Label("更多", systemImage: "ellipsis.circle")
            }
            .menuStyle(.button)
            .buttonStyle(.glass)
            .fixedSize()
            .help("从头部署：服务器忘掉所有设备，换一个新的加入链接（每台设备都要重新加入）；退出：这台 Mac 不再经它同步")
        }
        .disabled(model.flow != nil)
    }
}

/// 服务器上的一台设备：名字（这台 Mac 带个标记）、最近一次同步、什么时候加入的；别的设备右边是「移除…」
private struct HubDeviceLine: View {
    @Environment(AppModel.self) private var model
    let device: HubDevice
    let now: Double

    var body: some View {
        MachineLine(name: nonEmpty(device.name) ?? String(device.id.prefix(19)), detail: Texts.hubDeviceDetail(device, now: now)) {
            if device.me { Tag(text: "这台 Mac") }
        } trailing: {
            if !device.me {
                RowAction("移除…", systemImage: "xmark.circle") { model.removeHubDevice(device) }
                    .disabled(model.hubBusy)
                    .help("从服务器上移除这台设备：它现在的密钥作废（先问一句）")
            }
        }
    }
}

struct SharingRow: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 10) {
            SharingMark(size: 23).frame(width: 30, height: 30)
            VStack(alignment: .leading, spacing: 2) {
                Text("共享与同步").font(.body.weight(.medium))
                Text(subtitle).font(.caption).foregroundStyle(.plainSecondary).lineLimit(1)
            }
            Spacer(minLength: 4)
            if model.state?.config?.policy?.state == .pending {
                Tag(text: "等维护者", color: .orange)
            } else if model.ownHub?.state.failing == true {
                Tag(text: "没同步成", color: .orange)
            }
        }
        .sidebarRowPadding()
        .accessibilityElement(children: .combine)
    }

    /// 侧栏一行放得下的那句：维护者的共享开没开，加入了自己的同步服务器时再说一句（短写，免得截断）
    private var subtitle: String {
        let on = model.state?.config?.policy?.isOn == true
        guard model.ownHub != nil else { return on ? "维护者的共享开着" : "维护者的共享关着" }
        return (on ? "共享开着" : "共享关着") + " · 已加入同步服务器"
    }
}

/// “共享与同步”的图标：扣在一起的两个环。和用量统计、客户端更新的图标一套画法（粗圆的线，线宽是边长的 0.15，不带底）；
/// 颜色是 Logo 的青绿渐变到紫（左上角先掺一点白），和别的几个图标分得开。
struct SharingMark: View {
    var size: CGFloat = 22

    var body: some View {
        Canvas { ctx, s in
            let w = s.width
            var g = Path()
            g.addEllipse(in: CGRect(x: 0.10 * w, y: 0.27 * w, width: 0.46 * w, height: 0.46 * w))
            g.addEllipse(in: CGRect(x: 0.44 * w, y: 0.27 * w, width: 0.46 * w, height: 0.46 * w))
            let shade = Gradient(stops: [.init(color: Brand.teal.mix(with: .white, by: 0.25), location: 0),
                                         .init(color: Brand.teal, location: 0.35), .init(color: Brand.violet, location: 1)])
            ctx.stroke(g, with: .linearGradient(shade, startPoint: CGPoint(x: 0.15 * w, y: 0), endPoint: CGPoint(x: 0.85 * w, y: w)),
                       style: StrokeStyle(lineWidth: w * 0.15, lineCap: .round, lineJoin: .round))
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}
