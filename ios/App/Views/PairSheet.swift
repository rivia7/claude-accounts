import CcaRemoteKit
import SwiftUI
import VisionKit

/// 和一台电脑配对：扫它显示的二维码（里面有服务器的地址、证书的指纹和配对码，扫了就配好），或者粘贴它给的链接。
/// 经链接打开 App（系统相机扫的、在别处点的）时先问一句再配
struct PairSheet: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("和电脑配对")
                .navigationBarTitleDisplayMode(.inline)
                .demoSubtitle()
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button(done ? "完成" : "取消") { model.cancelPairing() }
                    }
                }
        }
        .interactiveDismissDisabled(working)
    }

    private var done: Bool {
        if case .done = model.pairing { return true }
        return false
    }

    private var working: Bool {
        if case .working = model.pairing { return true }
        return false
    }

    @ViewBuilder
    private var content: some View {
        switch model.pairing ?? .choosing {
        case .choosing: ChooseView()
        case .confirm(let inv): ConfirmView(invite: inv)
        case .working(let inv, let step): WorkingView(invite: inv, step: step)
        case .done(let p): DoneView(peer: p)
        case .failed(let inv, let message): FailedView(invite: inv, message: message)
        }
    }
}

/// 扫码（相机）和粘贴
private struct ChooseView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView {
            VStack(spacing: 18) {
                if Scanner.usable {
                    Scanner { model.scanned($0) }
                        .aspectRatio(1, contentMode: .fit)
                        .clipShape(RoundedRectangle(cornerRadius: 22, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 22, style: .continuous).strokeBorder(Surface.hairline))
                        .accessibilityLabel("相机：对准电脑上的二维码")
                    Text("对准电脑上「和手机配对」显示的二维码，识别后会自动开始配对。")
                        .font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
                } else {
                    Card("无法扫码", systemImage: "camera") {
                        Text(Scanner.why).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                }
                VStack(spacing: 8) {
                    PasteButton(payloadType: String.self) { items in
                        if let s = items.first { Task { @MainActor in model.scanned(s) } }
                    }
                    .labelStyle(.titleAndIcon)
                    .buttonBorderShape(.capsule)
                    Text("无法扫码时，可以在电脑上复制配对链接（终端运行 cca remote pair --web 输出的那一行），粘贴到这里。")
                        .font(.footnote).foregroundStyle(.secondary).multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Text("配对后，这台手机可以接入电脑上用 cca 启动的会话（电脑上可以随时改为只看或禁止接入）。内容端到端加密，中转服务器无法查看。")
                    .font(.footnote).foregroundStyle(.secondary).multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(20)
            .pageColumn()
        }
    }
}

/// 经链接打开的：和谁配、经哪台服务器，确认了再配
private struct ConfirmView: View {
    @Environment(AppModel.self) private var model
    let invite: PairInvite

    var body: some View {
        VStack(spacing: 22) {
            Spacer()
            Image(systemName: "desktopcomputer").font(.system(size: 44)).foregroundStyle(.secondary)
            VStack(spacing: 8) {
                Text("和「\(Texts.nonEmpty(invite.machine) ?? "这台电脑")」配对？").font(.title3.weight(.semibold)).multilineTextAlignment(.center)
                Text(serverLine).font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
                Text("请只扫描你自己电脑上显示的二维码。配对后，这台手机可以查看和操作那台电脑上的会话。")
                    .font(.footnote).foregroundStyle(.secondary).multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Button {
                model.pair(invite)
            } label: {
                Text("配对").frame(maxWidth: .infinity)
            }
            .glassButton(prominent: true)
            .controlSize(.large)
            Spacer()
        }
        .padding(24)
        .pageColumn()
    }

    private var serverLine: String {
        ["经 \(invite.server.host) 中转", invite.team.map { "Team「\($0)」" }].compactMap { $0 }.joined(separator: " · ")
    }
}

private struct WorkingView: View {
    let invite: PairInvite
    let step: PairStep

    var body: some View {
        VStack(spacing: 18) {
            Spacer()
            ProgressView().controlSize(.large)
            Text(text).font(.headline)
            Text("和「\(Texts.nonEmpty(invite.machine) ?? "电脑")」配对，经 \(invite.server.host) 中转").font(.callout).foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            Spacer()
        }
        .padding(24)
    }

    private var text: String {
        switch step {
        case .deriving: return "正在生成配对密钥…"
        case .asking: return "正在连接服务器…"
        case .confirming: return "等待电脑确认…"
        }
    }
}

private struct DoneView: View {
    @Environment(AppModel.self) private var model
    let peer: Peer

    var body: some View {
        VStack(spacing: 18) {
            Spacer()
            Image(systemName: "checkmark.circle.fill").font(.system(size: 52)).foregroundStyle(.green)
            Text("已与「\(peer.name)」配对").font(.title3.weight(.semibold)).multilineTextAlignment(.center)
            Text("电脑上用 cca 启动的会话会显示在「远程会话」中。现在可以关闭电脑上的配对窗口了。")
                .font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
            Button {
                model.cancelPairing()
            } label: {
                Text("查看会话").frame(maxWidth: .infinity)
            }
            .glassButton(prominent: true)
            .controlSize(.large)
            Spacer()
        }
        .padding(24)
        .pageColumn()
    }
}

private struct FailedView: View {
    @Environment(AppModel.self) private var model
    let invite: PairInvite?
    let message: String

    var body: some View {
        VStack(spacing: 18) {
            Spacer()
            Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 44)).foregroundStyle(.orange)
            Text("配对失败").font(.title3.weight(.semibold))
            Text(message).font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            Button {
                model.startPairing()
            } label: {
                Text("重新扫码").frame(maxWidth: .infinity)
            }
            .glassButton(prominent: true)
            .controlSize(.large)
            Spacer()
        }
        .padding(24)
        .pageColumn()
    }
}

/// 相机扫二维码（VisionKit）：只认二维码，扫到一个就交出去、不再扫
struct Scanner: UIViewControllerRepresentable {
    let found: (String) -> Void

    static var usable: Bool {
        DataScannerViewController.isSupported && DataScannerViewController.isAvailable
    }

    static var why: String {
        if !DataScannerViewController.isSupported { return "这台设备无法使用相机扫码，请使用下方的「粘贴」。" }
        return "无法使用相机：请在「设置」→ cca 中打开「相机」，或使用下方的「粘贴」。"
    }

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let vc = DataScannerViewController(recognizedDataTypes: [.barcode(symbologies: [.qr])], qualityLevel: .balanced,
                                           recognizesMultipleItems: false, isHighFrameRateTrackingEnabled: false,
                                           isHighlightingEnabled: true)
        vc.delegate = context.coordinator
        try? vc.startScanning()
        return vc
    }

    func updateUIViewController(_ vc: DataScannerViewController, context: Context) {}

    static func dismantleUIViewController(_ vc: DataScannerViewController, coordinator: Coordinator) {
        vc.stopScanning()
    }

    func makeCoordinator() -> Coordinator { Coordinator(found) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let found: (String) -> Void
        private var done = false

        init(_ found: @escaping (String) -> Void) {
            self.found = found
        }

        func dataScanner(_ s: DataScannerViewController, didAdd items: [RecognizedItem], allItems: [RecognizedItem]) {
            guard !done else { return }
            for item in items {
                if case .barcode(let b) = item, let text = b.payloadStringValue {
                    done = true
                    s.stopScanning()
                    found(text)
                    return
                }
            }
        }
    }
}
