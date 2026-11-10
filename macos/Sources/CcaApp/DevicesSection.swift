import SwiftUI

/// 账号页的「设备」：映射里的这个账号现在在哪些设备上用（维护者的服务器上的记录，cca 查好、把时间换成这台 Mac 的）。
/// 这台 Mac 排第一，其余按 在用、闲置、离线、没在用。在用是开着会话、10 分钟内有动静；闲置是开着却很久没动静；
/// 离线是开着会话却很久没回报（可能休眠、断网）；没在用的写上次什么时候用。账号页看得见时一分钟查一次（AppModel），
/// 标题右边是查到的一句话、多久以前查的和「刷新」。只有 cca 说这个账号有（`can.devices`）时才显示。
struct DevicesSection: View {
    @Environment(AppModel.self) private var model
    @Environment(Guide.self) private var guide
    let account: Account

    var body: some View {
        let snap = model.devices[account.ref]
        let loading = model.devicesLoading.contains(account.ref)
        VStack(alignment: .leading, spacing: 12) {
            SectionHeader("设备") {
                TimelineView(.periodic(from: .now, by: 30)) { context in
                    freshness(snap, loading: loading, now: context.date.timeIntervalSince1970)
                }
                Button {
                    model.refreshDevices(account.ref, manual: true)
                } label: {
                    Label(loading ? "刷新中…" : "刷新", systemImage: "arrow.clockwise")
                }
                .buttonStyle(.glass)
                .disabled(loading)
                .help("问一次维护者的服务器：这个账号现在在哪些设备上用")
            }
            TimelineView(.periodic(from: .now, by: 30)) { context in
                content(snap, loading: loading, now: context.date.timeIntervalSince1970)
            }
        }
    }

    @ViewBuilder
    private func freshness(_ s: DevicesSnapshot?, loading: Bool, now: Double) -> some View {
        if loading, s?.fetchedEpoch == nil {
            ProgressView().controlSize(.small)
        } else if let rows = s?.account?.devices, let at = s?.fetchedEpoch {
            Text(Texts.devicesSummary(rows) + " · " + Texts.agoPhrase(now - at) + "查询")
                .font(.caption)
                .foregroundStyle(.secondary)
                .monospacedDigit()
        }
    }

    @ViewBuilder
    private func content(_ s: DevicesSnapshot?, loading: Bool, now: Double) -> some View {
        let label = model.appLabel(account.tool)
        VStack(alignment: .leading, spacing: 9) {
            if let rows = s?.account?.devices {
                if rows.isEmpty {
                    Label("30 天内没有设备经 cca 用过这个账号", systemImage: "desktopcomputer")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(rows) { d in
                        DeviceLine(device: d, appLabel: label, now: now)
                    }
                }
            } else if !loading, s?.error == nil {
                Text("还没查过").font(.callout).foregroundStyle(.secondary)
            }
            if let e = s?.error {
                Label(e, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let e = s?.doc?.reportingError {
                Label("这台 Mac 最近一次没报成：" + e, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if guide.showDetails, let doc = s?.doc, let idle = doc.idleAfter, let offline = doc.offlineAfter {
                Text("经 cca 打开的命令行会话和 \(label) 算在用；\(Texts.span(idle))没动静算闲置（命令行看终端的最后输入、输出，App 看它用的 CPU）；开着会话却\(Texts.span(offline))没回报算离线。设备名是维护者给各台机器的 deploy key 起的别名" + (nonEmpty(doc.me).map { "，这台 Mac 叫 \($0)" } ?? ""))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.horizontal, 4)
    }
}

/// 一台设备：状态点、名字（这台 Mac 带个标签）、在干什么 / 上次什么时候用。
struct DeviceLine: View {
    let device: DeviceRow
    let appLabel: String
    let now: Double

    var body: some View {
        let text = Texts.deviceState(device, appLabel: appLabel, now: now)
        let offline = device.state == .offline ? Texts.deviceOfflineHelp(device, now: now) : ""
        HStack(spacing: 10) {
            DeviceDot(state: device.state)
            Text(device.name).fontWeight(.medium).lineLimit(1)
            if device.isMe { Tag(text: "这台 Mac") }
            Spacer(minLength: 8)
            Text(text)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
                .monospacedDigit()
        }
        .font(.callout)
        .help(offline.isEmpty ? text : offline)
        .accessibilityElement(children: .combine)
        .accessibilityHint(offline)
    }
}

/// 状态点：在用是实心的绿点，闲置是绿圈，离线是灰圈，没在用是浅灰点（颜色只表达状态）。
struct DeviceDot: View {
    let state: DeviceState

    var body: some View {
        Group {
            switch state {
            case .inUse: Circle().fill(.green)
            case .idle: Circle().strokeBorder(.green, lineWidth: 1.5)
            case .offline: Circle().strokeBorder(.secondary, lineWidth: 1.5)
            default: Circle().fill(.quaternary)
            }
        }
        .frame(width: 8, height: 8)
        .accessibilityHidden(true)
    }
}
