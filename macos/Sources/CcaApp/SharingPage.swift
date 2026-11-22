import SwiftUI

/// 共享与同步：Team（PROTOCOL 4.14，`TeamsCard`）给这台设备的账号，和以前的维护者共享（维护者的映射）的开关。状态、账号数都照 cca 给的
/// （`config.teams`、`config.policy`），不在界面里重算；加入、新建、部署、打开共享是提问会话，退出、关掉共享由 cca 先问同意。
struct SharingPage: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                heading
                if model.teamAvailable { TeamsCard() }
                if model.policyCardShown {
                    VStack(alignment: .leading, spacing: 10) {
                        policyCard
                        Text(model.teamAvailable
                             ? "维护者共享的账号改经 Team 给：维护者在映射里给这台 Mac 发了邀请的话，拉到映射以后它自己加入维护者的 Team。共享关着时，这台 Mac 不拉维护者的映射、不往维护者的服务器上报。"
                             : "共享关着时，这台 Mac 只用你自己的账号：不生成给维护者的公钥、不拉维护者的映射、不往维护者的服务器上报。远程会话经维护者的服务器中转，要先打开共享。")
                            .font(.caption).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
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
                if model.sharingBusy || model.teamBusy { ProgressView().controlSize(.small) }
            }
            Text(model.teamAvailable ? "Team 给这台 Mac 的账号：管理员定好每个账号走哪个节点、给哪些设备；设备统计、折合金额经这个 Team 的服务器在几台设备之间同步。自己的几台设备也可以建一个 Team，经它同步自己的账号。"
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
            } else if model.teams.contains(where: { $0.state.failing }) {
                Tag(text: "Team 要处理", color: .orange)
            }
        }
        .sidebarRowPadding()
        .accessibilityElement(children: .combine)
    }

    /// 侧栏一行放得下的那句（短写，免得截断）
    private var subtitle: String {
        Texts.sharingSubtitle(policyOn: model.state?.config?.policy?.isOn == true, teams: model.teamAvailable ? model.teams.map(\.name) : nil)
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
