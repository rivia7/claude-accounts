import CcaRemoteKit
import SwiftUI

/// 新建会话（远程启动，docs/dev/ios.md「新建会话（远程启动）」）：那台电脑上写好的模板（工具、账号、文件夹、参数）挑一个，能带
/// 开场白的可以写一句，点「启动」：那台电脑起一个新的会话，起来了这一页关掉、打开它的终端。模板在那台电脑上加、改
/// （cca remote template），这里只能挑
struct LaunchSheet: View {
    @Environment(AppModel.self) private var model
    let peer: Peer
    /// 选着的模板的编号（列表刷新以后摘要可能变了：启动时照编号拿现在的那一份）
    @State private var picked: String?
    @State private var line = ""
    @State private var working: Task<Void, Never>?
    @State private var failure: String?

    private var launcher: HostLauncher? { model.launcher(peer) }
    private var templates: [LaunchTemplate] { launcher?.info.templates ?? [] }
    private var template: LaunchTemplate? { templates.first { $0.id == picked } }

    var body: some View {
        NavigationStack {
            List {
                if launcher?.online == false {
                    Section {
                        Label("那台电脑这会儿收不到启动的请求（睡着了、没联网，或者远程启动的后台进程没在运行），现在起不了。", systemImage: "moon.zzz")
                            .font(.callout).foregroundStyle(.secondary)
                    }
                }
                Section {
                    if launcher == nil {
                        Text("那台电脑现在不让这台手机远程启动会话。").font(.callout).foregroundStyle(.secondary)
                    } else if templates.isEmpty {
                        Text("那台电脑上还没有模板。在 Mac 上打开 cca App 的「远程会话」，在「远程启动」里点「新建模板…」（终端里是 cca remote template add），它就会列在这里。")
                            .font(.callout).foregroundStyle(.secondary)
                    } else {
                        ForEach(templates) { t in
                            TemplateRow(template: t, account: account(t), picked: t.id == picked) {
                                picked = t.id
                                failure = nil
                            }
                            .disabled(working != nil)
                        }
                    }
                } header: {
                    HStack(spacing: 8) {
                        Image(systemName: "desktopcomputer").foregroundStyle(.secondary).accessibilityHidden(true)
                        Text(peer.name).font(.subheadline.weight(.semibold)).foregroundStyle(.primary).textCase(nil).lineLimit(1)
                        if let team = peer.team { Tag(text: team) }
                    }
                } footer: {
                    if let more = launcher?.info.more, more > 0 {
                        Text("还有 \(more) 个模板这里放不下")
                    }
                }
                if let t = template, t.line {
                    Section {
                        TextField("会话起来以后先说的一句话", text: $line, axis: .vertical)
                            .lineLimit(1...5)
                            .disabled(working != nil)
                    } header: {
                        Text("开场白（可以不写）")
                    } footer: {
                        Text(lineNote(t))
                    }
                }
                if let failure {
                    Section {
                        Label(failure, systemImage: "exclamationmark.triangle")
                            .font(.callout).foregroundStyle(.orange)
                    }
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle("新建会话")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { close() }
                }
            }
            .safeAreaBar(edge: .bottom) { bar }
        }
        .interactiveDismissDisabled(working != nil)
        .onAppear {
            if picked == nil { picked = templates.first?.id }
        }
        .onChange(of: templates.map(\.id)) { _, ids in
            // 刷新以后选着的那个没了：不替人选别的
            if let p = picked, !ids.contains(p) { picked = nil }
        }
    }

    private var bar: some View {
        VStack(spacing: 8) {
            Button {
                start()
            } label: {
                HStack(spacing: 8) {
                    if working != nil { ProgressView() }
                    Text(working == nil ? "启动" : "正在启动…")
                }
                .frame(maxWidth: .infinity)
            }
            .glassButton(prominent: true)
            .controlSize(.large)
            .disabled(!ready)
            Text(working == nil ? "在「\(peer.name)」上起一个新的会话，起来了就打开它的终端" : "等「\(peer.name)」回话，最多 20 秒")
                .font(.footnote).foregroundStyle(.secondary).multilineTextAlignment(.center)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 10)
        .pageColumn()
    }

    /// 能启动：选了模板、启动器在线、开场白（写了的话）发得出去、没在等
    private var ready: Bool {
        guard working == nil, let t = template, launcher?.online == true else { return false }
        guard t.line, let said = Self.opening(line) else { return true }
        return LaunchAsk.lineOK(said)
    }

    private func lineNote(_ t: LaunchTemplate) -> String {
        let n = Self.opening(line)?.unicodeScalars.count ?? 0
        if n > LaunchAsk.lineMax { return "太长了：\(n) 个字，最多 \(LaunchAsk.lineMax) 个" }
        return "会话起来以后当作头一句话发给 \(Texts.cliProduct(t.tool) ?? "它")。最多 \(LaunchAsk.lineMax) 个字，换行会换成空格"
    }

    private func start() {
        guard ready, let t = template else { return }
        failure = nil
        let said = t.line ? Self.opening(line) : nil
        working = Task {
            let why = await model.launch(t, on: peer, line: said)
            guard !Task.isCancelled else { return }
            working = nil
            failure = why?.text
        }
    }

    /// 取消：还在等回话的不等了（那台电脑可能已经起了：会列在会话里）
    private func close() {
        working?.cancel()
        working = nil
        model.launchFor = nil
    }

    /// 那台电脑放上来的账号里，模板用的那个（头像用）
    private func account(_ t: LaunchTemplate) -> AccountStatus? {
        guard !t.aid.isEmpty else { return nil }
        return model.status(peer)?.accounts.first { $0.aid == t.aid }
    }

    /// 开场白：换行、制表符这些控制字符换成空格，去掉两头的空白；什么都没写是不带
    static func opening(_ s: String) -> String? {
        var u = String.UnicodeScalarView()
        for c in s.unicodeScalars {
            u.append(c.properties.generalCategory == .control ? " " : c)
        }
        let t = String(u).trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? nil : t
    }
}

/// 一个模板：账号的头像（那台电脑放上来的账号里有它时）、名字、哪个工具哪个账号、文件夹、参数；点一下选它
private struct TemplateRow: View {
    let template: LaunchTemplate
    let account: AccountStatus?
    let picked: Bool
    let pick: () -> Void

    var body: some View {
        Button(action: pick) {
            HStack(alignment: .top, spacing: 12) {
                mark
                VStack(alignment: .leading, spacing: 3) {
                    Text(Texts.nonEmpty(template.name) ?? Texts.templateTool(template))
                        .font(.body.weight(.medium)).foregroundStyle(.primary).lineLimit(1)
                    Text(Texts.templateTool(template)).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    if let d = Texts.nonEmpty(template.dir) {
                        Text(d).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                    }
                    if let x = Texts.nonEmpty(template.args) {
                        Text(x).font(.caption2.monospaced()).foregroundStyle(.secondary).lineLimit(2)
                    }
                }
                Spacer(minLength: 6)
                Image(systemName: "checkmark").font(.body.weight(.semibold)).foregroundStyle(.tint)
                    .opacity(picked ? 1 : 0)
                    .accessibilityHidden(true)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(picked ? .isSelected : [])
    }

    @ViewBuilder
    private var mark: some View {
        if let account {
            AccountAvatar(account: account, size: 30)
        } else {
            Image(systemName: "terminal")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(.white)
                .frame(width: 30, height: 30)
                .background(Circle().fill(Color(uiColor: .systemGray).opacity(0.6)))
                .accessibilityHidden(true)
        }
    }
}
