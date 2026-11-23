import SwiftUI

/// 共享与同步：Team（PROTOCOL 4.14，`TeamsCard`）给这台设备的账号。只在 cca 有 Team（`hello.team`）时才有这一页；状态、账号数都照
/// cca 给的（`config.teams`），不在界面里重算；加入、新建、部署是提问会话，退出由 cca 先问同意。
struct SharingPage: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 22) {
                heading
                if model.teamAvailable { TeamsCard() }
            }
            .padding(.top, 24).padding(.bottom, 36).pageColumn()
        }
        .navigationTitle("共享与同步")
    }

    private var heading: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                Text("共享与同步").font(.system(size: 24, weight: .semibold))
                if model.teamBusy { ProgressView().controlSize(.small) }
            }
            Text("Team 给这台 Mac 的账号：管理员定好每个账号走哪个节点、给哪些设备；设备统计、折合金额经这个 Team 的服务器在几台设备之间同步。自己的几台设备也可以建一个 Team，经它同步自己的账号。")
                .font(.callout).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
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
                Text(Texts.sharingSubtitle(teams: model.teams.map(\.name))).font(.caption).foregroundStyle(.plainSecondary).lineLimit(1)
            }
            Spacer(minLength: 4)
            if model.teams.contains(where: { $0.state.failing }) {
                Tag(text: "Team 要处理", color: .orange)
            }
        }
        .sidebarRowPadding()
        .accessibilityElement(children: .combine)
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
