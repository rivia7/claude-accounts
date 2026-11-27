import CcaRemoteKit
import SwiftUI

/// 新建会话（远程启动，docs/dev/ios.md「新建会话（远程启动）」）：那台电脑上写好的模板（工具、文件夹、参数）挑一个，再挑用哪个账号起
/// （3.7.10 起的 cca 不把账号写在模板里，说明里列着能挑的账号；旧版的 cca 的模板自己定了账号，不用挑），能带开场白的可以写一句，
/// 点「启动」：那台电脑起一个新的会话，起来了这一页关掉、打开它的终端。模板在那台电脑上加、改（cca remote template），这里只能挑
struct LaunchSheet: View {
    @Environment(AppModel.self) private var model
    let peer: Peer
    /// 选着的模板的编号（列表刷新以后摘要可能变了：启动时照编号拿现在的那一份）
    @State private var picked: String?
    /// 选着的账号的编号（说明里能挑的账号的 i：按配对算的，和「账号」页里同一个账号的编号一样）
    @State private var chosen: String?
    @State private var line = ""
    @State private var working: Task<Void, Never>?
    @State private var failure: String?

    private var launcher: HostLauncher? { model.launcher(peer) }
    private var templates: [LaunchTemplate] { launcher?.info.templates ?? [] }
    private var template: LaunchTemplate? { templates.first { $0.id == picked } }
    /// 那台电脑的 cca 让起的时候挑账号（说明里有 accounts，3.7.10 起）
    private var picksAccounts: Bool { launcher?.info.accounts != nil }
    /// 选着的模板能挑的账号（它那个工具的）；nil：还没选模板，或者那台电脑的 cca 是旧版的（用模板自己定的账号）
    private var choices: [LaunchAccount]? { template.flatMap { launcher?.info.accounts(for: $0) } }
    private var account: LaunchAccount? { choices?.first { $0.id == chosen } }

    var body: some View {
        NavigationStack {
            List {
                if launcher?.online == false {
                    Section {
                        Label("电脑暂时收不到启动请求（可能已休眠、断网，或远程启动的后台进程没有运行），现在无法启动。", systemImage: "moon.zzz")
                            .font(.callout).foregroundStyle(.secondary)
                    }
                }
                Section {
                    if launcher == nil {
                        Text("电脑不允许这台手机远程启动会话。").font(.callout).foregroundStyle(.secondary)
                    } else if templates.isEmpty {
                        Text("电脑上还没有模板。请在 Mac 上打开 cca App 的「远程会话」，在「远程启动」中点「新建模板…」（或在终端运行 cca remote template add），模板就会显示在这里。")
                            .font(.callout).foregroundStyle(.secondary)
                    } else {
                        ForEach(templates) { t in
                            TemplateRow(template: t, account: picksAccounts ? nil : templateAccount(t), withAccount: !picksAccounts,
                                        picked: t.id == picked) {
                                pick(t.id)
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
                        Text("另有 \(more) 个模板未显示")
                    }
                }
                if let t = template, let list = choices {
                    Section {
                        if list.isEmpty {
                            Text(Texts.launchNoAccount(t.tool)).font(.callout).foregroundStyle(.secondary)
                        } else {
                            let now = Date().timeIntervalSince1970
                            ForEach(list) { a in
                                PickRow(account: a, status: status(of: a), picked: a.id == chosen, now: now) {
                                    chosen = a.id
                                    failure = nil
                                }
                                .disabled(working != nil)
                            }
                        }
                    } header: {
                        Text("用哪个账号")
                    }
                }
                if let t = template, t.line {
                    Section {
                        TextField("会话启动后发送的第一句话", text: $line, axis: .vertical)
                            .lineLimit(1...5)
                            .disabled(working != nil)
                    } header: {
                        Text("开场白（选填）")
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
            if picked == nil { pick(templates.first?.id) }
        }
        .onChange(of: templates.map(\.id)) { _, ids in
            // 刷新以后选着的那个没了：不替人选别的
            if let p = picked, !ids.contains(p) { picked = nil }
            settle()
        }
        .onChange(of: launcher?.info.accounts?.map(\.id) ?? []) { _, _ in settle() }
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
            Text(barNote)
                .font(.footnote).foregroundStyle(.secondary).multilineTextAlignment(.center)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 10)
        .pageColumn()
    }

    private var barNote: String {
        if working != nil { return "等待「\(peer.name)」响应，最多 20 秒" }
        if let a = account { return "用「\(Texts.nonEmpty(a.name) ?? "账号")」在「\(peer.name)」上启动新会话，启动后会打开它的终端" }
        if choices?.isEmpty == false { return "选择用哪个账号启动" }
        return "在「\(peer.name)」上启动新会话，启动后会打开它的终端"
    }

    /// 能启动：选了模板（要挑账号的也挑了）、启动器在线、开场白（写了的话）发得出去、没在等
    private var ready: Bool {
        guard working == nil, let t = template, launcher?.online == true else { return false }
        if choices != nil, account == nil { return false }
        guard t.line, let said = Self.opening(line) else { return true }
        return LaunchAsk.lineOK(said)
    }

    private func lineNote(_ t: LaunchTemplate) -> String {
        let n = Self.opening(line)?.unicodeScalars.count ?? 0
        if n > LaunchAsk.lineMax { return "太长了：\(n) 个字（最多 \(LaunchAsk.lineMax) 个）" }
        return "会话启动后作为第一句话发给 \(Texts.cliProduct(t.tool) ?? "它")。最多 \(LaunchAsk.lineMax) 个字，换行会替换为空格"
    }

    /// 选一个模板（挑的账号跟着看还能不能用，见 settle）
    private func pick(_ id: String?) {
        picked = id
        failure = nil
        settle()
    }

    /// 挑的账号不在现在能挑的里面了（换了别的工具的模板、刷新以后没了）：不选了；能挑的只有一个时替人选上
    private func settle() {
        let list = choices ?? []
        if let c = chosen, !list.contains(where: { $0.id == c }) { chosen = nil }
        if chosen == nil, list.count == 1 { chosen = list[0].id }
    }

    private func start() {
        guard ready, let t = template else { return }
        failure = nil
        let said = t.line ? Self.opening(line) : nil
        let who = account
        working = Task {
            let why = await model.launch(t, on: peer, as: who, line: said)
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

    /// 旧版的 cca：那台电脑放上来的账号里，模板定的那个（头像用）
    private func templateAccount(_ t: LaunchTemplate) -> AccountStatus? {
        guard !t.aid.isEmpty else { return nil }
        return model.status(peer)?.accounts.first { $0.aid == t.aid }
    }

    /// 那台电脑放上来的账号里，能挑的这一个（头像、现在怎么样）：编号是同一个（都按这个配对算），对不上的再照账号摘要找
    private func status(of a: LaunchAccount) -> AccountStatus? {
        let all = model.status(peer)?.accounts ?? []
        return all.first { $0.id == a.id } ?? (a.aid.isEmpty ? nil : all.first { $0.aid == a.aid })
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

/// 一个模板：账号的头像（模板自己定了账号、那台电脑放上来的账号里有它时）、名字、哪个工具（模板自己定了账号的还写上哪个账号）、
/// 文件夹、参数；点一下选它
private struct TemplateRow: View {
    let template: LaunchTemplate
    let account: AccountStatus?
    /// 模板自己定了账号（旧版的 cca）：第二行写上账号
    let withAccount: Bool
    let picked: Bool
    let pick: () -> Void

    var body: some View {
        Button(action: pick) {
            HStack(alignment: .top, spacing: 12) {
                mark
                VStack(alignment: .leading, spacing: 3) {
                    Text(Texts.nonEmpty(template.name) ?? Texts.templateTool(template, withAccount: withAccount))
                        .font(.body.weight(.medium)).foregroundStyle(.primary).lineLimit(1)
                    Text(Texts.templateTool(template, withAccount: withAccount)).font(.caption).foregroundStyle(.secondary).lineLimit(1)
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
            Glyph(systemName: "terminal")
        }
    }
}

/// 能挑的一个账号：头像、名字（那台电脑上的叫法）和套餐、现在怎么样（那台电脑放上来的账号里有它时：额度用完了、建议先用……）；
/// 点一下选它
private struct PickRow: View {
    let account: LaunchAccount
    let status: AccountStatus?
    let picked: Bool
    let now: Double
    let pick: () -> Void

    var body: some View {
        Button(action: pick) {
            HStack(spacing: 12) {
                if let status {
                    AccountAvatar(account: status, size: 30)
                } else {
                    Glyph(systemName: "person.fill")
                }
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(Texts.nonEmpty(account.name) ?? "账号").font(.body.weight(.medium)).foregroundStyle(.primary).lineLimit(1)
                        if let s = status, !s.plan.isEmpty { PlanBadge(plan: s.plan, tier: s.tier) }
                    }
                    if let s = status {
                        MoodLine(mood: AccountMood(s, now: now), prefix: s.isAPI ? "第三方 API" : nil, now: now)
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
}

/// 没有头像时的灰圆（模板、不在「账号」页里的账号）
private struct Glyph: View {
    let systemName: String

    var body: some View {
        Image(systemName: systemName)
            .font(.system(size: 13, weight: .semibold))
            .foregroundStyle(.white)
            .frame(width: 30, height: 30)
            .background(Circle().fill(Color(uiColor: .systemGray).opacity(0.6)))
            .accessibilityHidden(true)
    }
}
