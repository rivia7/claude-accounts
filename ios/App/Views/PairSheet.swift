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
                    Text("对准电脑上「和手机配对」的二维码，扫到就开始配对。")
                        .font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
                } else {
                    Card("扫不了码", systemImage: "camera") {
                        Text(Scanner.why).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                }
                VStack(spacing: 8) {
                    PasteButton(payloadType: String.self) { items in
                        if let s = items.first { Task { @MainActor in model.scanned(s) } }
                    }
                    .labelStyle(.titleAndIcon)
                    .buttonBorderShape(.capsule)
                    Text("没法扫码时：在电脑上复制配对链接（终端里 cca remote pair --web 打出来的那一行），在这里粘贴。")
                        .font(.footnote).foregroundStyle(.secondary).multilineTextAlignment(.center)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Text("配对以后，这台手机能接入那台电脑上用 cca 启动的会话（那台电脑上可以随时改成只看或者不让接入）。内容端到端加密，中转的服务器看不到。")
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
                Text("只和自己电脑上显示的二维码配对。配对以后这台手机能看和操作那台电脑上的会话。")
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
        case .deriving: return "正在算配对的密钥…"
        case .asking: return "正在问服务器…"
        case .confirming: return "等电脑确认…"
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
            Text("和「\(peer.name)」配好了").font(.title3.weight(.semibold)).multilineTextAlignment(.center)
            Text("它上面用 cca 启动的会话会列在「远程会话」里。电脑上的配对窗口可以关了。")
                .font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
            Button {
                model.cancelPairing()
            } label: {
                Text("看会话").frame(maxWidth: .infinity)
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
            Text("没配对成功").font(.title3.weight(.semibold))
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
        if !DataScannerViewController.isSupported { return "这台设备不能用相机扫码。用下面的「粘贴」。" }
        return "相机用不了：在「设置」→ cca 里打开「相机」，或者用下面的「粘贴」。"
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
