import Foundation

private let teamA = "5d0c2a9be17f4c3e8a6b1d2f3e4c5a6b"
private let teamB = "c7e1a4b2d39f40718e5a6c7d8b9f0a1e"

/// Team（PROTOCOL 2.1 的 `hello.team`，2.2 的 `config.teams`、账号的 `team`，4.14 的 `cca team`）：只在 --full（手写的 Tests/Fixtures）里跑
func teamFixtureTests() {
    section("Team")
    if let h = loadFixture(HelloDoc.self, "hello.json") {
        check("hello：team（cca 有 Team）", h.team == 1 && h.team == TeamAPI.version)
    }
    if let h = loadFixture(HelloDoc.self, "hello_nocodex_notinstalled.json") {
        check("hello：旧版 cca 没有 team", h.team == nil)
    }
    if let s = loadFixture(StateDoc.self, "state.json") {
        let teams = s.config?.teams ?? []
        check("state：config.teams 照加入的先后，编号是空的不算", teams.map(\.name) == ["书房组", "设计组"] && teams.map(\.id) == [teamA, teamB])
        check("state：这台设备是书房组的管理员，配置同步着、给了它 3 个账号",
              teams.first?.isAdmin == true && teams.first?.state == .ok && teams.first?.state.failing == false && teams.first?.seq == 4
              && teams.first?.accounts == 3 && teams.first?.host == "team.example.com:18445" && teams.first?.device == "studio-mac"
              && teams.first?.joinedEpoch == 1793900000)
        check("state：被设计组的管理员移除了（要用户知道，有 cca 给的原因）",
              teams.last?.isAdmin == false && teams.last?.state == .removed && teams.last?.state.failing == true
              && teams.last?.error?.contains("被「设计组」的管理员移除了") == true)
        let carol = s.account(AccountRef(tool: .claude, key: "carol@example.com"))
        let bob = s.account(AccountRef(tool: .claude, key: "工作(bob@example.com)"))
        let dave = s.account(AccountRef(tool: .claude, key: "测试(dave@example.com)"))
        check("state：bob、carol 是书房组给的（bob 经它的服务器同步；carol 的 hub 是 nil，team 照样有）；dave 不是哪个 Team 给的",
              bob?.team == teamA && bob?.hub == .team && carol?.team == teamA && carol?.hub == nil && dave != nil && dave?.team == nil)
        check("state：carol 的别名被书房组的管理员锁定了（不能改）；dave 的没有；旧版 cca 没有这一项是没锁定",
              carol?.aliasLocked == true && dave?.aliasLocked == false
              && loadFixture(StateDoc.self, "state_cost_sync.json")?.allAccounts.allSatisfy { !$0.aliasLocked } == true)
        let all = s.allAccounts
        check("按 Team 筛选：全部、某个 Team 的、不在 Team 里的",
              all.filter(TeamFilter.all.includes).count == all.count && all.filter(TeamFilter.team(teamA).includes).map(\.key) == ["工作(bob@example.com)", "carol@example.com", "grace@example.com"]
              && all.filter(TeamFilter.team(teamB).includes).isEmpty && all.filter(TeamFilter.outside.includes).count == all.count - 3)
        check("筛选项的名字：Team 的名字；不在了的 Team 写编号的开头", Texts.teamFilter(.all, teams: teams) == "全部账号"
              && Texts.teamFilter(.team(teamA), teams: teams) == "书房组" && Texts.teamFilter(.team("0123456789abcdef"), teams: teams) == "01234567"
              && Texts.teamFilter(.outside, teams: teams) == "不在 Team 里的")
        check("Team 的状态那一句照 cca 给的说（不说第几版）", Texts.teamState(teams[0]) == "配置已同步" && Texts.teamState(teams[1]).contains("被「设计组」的管理员移除了")
              && Texts.teamSummary(teams[0]) == "这台设备叫 studio-mac · 给了这台设备 3 个账号 · team.example.com:18445")
    }
    if let s = loadFixture(StateDoc.self, "state_cost_sync.json") {
        check("旧版 cca 的 state（没有 config.teams、账号的 team）：不在任何 Team 里", s.config?.teams.isEmpty == true && s.allAccounts.allSatisfy { $0.team == nil })
    }
    check("筛选记在设置里的写法：来回不变；不认识的当全部",
          [TeamFilter.all, .team(teamA), .outside].allSatisfy { TeamFilter(stored: $0.stored) == $0 } && TeamFilter(stored: nil) == .all
          && TeamFilter(stored: "later") == .all && TeamFilter(stored: "team:") == .all)
    let twoTools = #"{"team":"ab","accounts":{"claude":{"x@example.com":"US-A"},"codex":{"x@example.com":"US-A"}},"aliases":{"x@example.com":"team-3"}"#
    let perTool = try? JSONDecoder().decode(TeamDevicesDoc.self, from: Data((twoTools + #","tool_aliases":{"claude":{"x@example.com":"team-3"},"codex":{"x@example.com":"team-1"}}}"#).utf8))
    let flatOnly = try? JSONDecoder().decode(TeamDevicesDoc.self, from: Data((twoTools + "}").utf8))
    check("team devices：旧版 cca 没有 alias_lock 时是 nil（App 不给锁定的开关）", flatOnly != nil && flatOnly?.aliasLock == nil && perTool?.aliasLock == nil)
    check("team devices：别名按工具（同一个邮箱在 Claude、Codex 里叫得不一样）；旧版 cca 没有 tool_aliases 时用按邮箱的",
          perTool?.accountList.map(\.alias) == ["team-3", "team-1"] && flatOnly?.accountList.map(\.alias) == ["team-3", "team-3"])
    let mixed = #"{"team":"ab","accounts":{"codex":{"z@example.com":"US-A"},"claude":{"a@example.com":"US-A","b@example.com":"US-B","c@example.com":"US-A","d@example.com":"US-A","e@example.com":"US-A"}},"aliases":{},"tool_aliases":{"claude":{"a@example.com":"team-10","b@example.com":"team-2","d@example.com":"Work","e@example.com":"team-1"},"codex":{"z@example.com":"team-1"}}}"#
    check("team devices：账号先照工具、再照别名排（team-2 在 team-10 前面），没有别名的在后",
          (try? JSONDecoder().decode(TeamDevicesDoc.self, from: Data(mixed.utf8)))?.accountList.map(\.spec)
            == ["claude:e@example.com", "claude:b@example.com", "claude:a@example.com", "claude:d@example.com", "claude:c@example.com", "codex:z@example.com"])
    let co = try? JSONDecoder().decode(TeamEntry.self, from: Data(#"{"id":"ab","name":"x","role":"member","coadmin":true,"state":"ok"}"#.utf8))
    check("别的管理员（3.6.15 起 coadmin）：能管理、不是建 Team 的那台；旧版 cca 没有这一项是成员",
          co?.isAdmin == true && co?.isOwner == false && loadFixture(StateDoc.self, "state.json")?.config?.teams.first?.isOwner == true
          && loadFixture(StateDoc.self, "state.json")?.config?.teams.last?.isAdmin == false)
    check("命令：授予、收回管理员身份；同意项、结果码",
          CcaCommand.teamAdmin(team: teamA, device: "0a1b", add: true).arguments == ["team", "admin", teamA, "add", "0a1b"]
          && CcaCommand.teamAdmin(team: teamA, device: "0a1b", add: false).arguments == ["team", "admin", teamA, "remove", "0a1b"]
          && !CcaCommand.teamAdmin(team: teamA, device: "0a1b", add: true).runsToCompletion
          && ConsentItem.allKnown.contains(.teamAdminAdd) && ConsentItem.allKnown.contains(.teamAdminRemove)
          && ResultCode.allKnown.contains(.teamAdminAdded) && ResultCode.allKnown.contains(.teamAdminRemoved))
    let unknown = try? JSONDecoder().decode(TeamEntry.self, from: Data(#"{"id":"ab","name":"x","role":"later-role","state":"later-state"}"#.utf8))
    check("不认识的角色当成员，不认识的状态当出了问题", unknown?.isAdmin == false && unknown?.state.failing == true && unknown?.seq == 0 && unknown?.accounts == 0)
    if let doc = loadFixture(TeamDevicesDoc.self, "team_devices.json") {
        let now = 1794042000.0
        check("team devices：Team 的编号和名字、配置里的账号（照工具、别名排）和别名、带着定义的节点", doc.team == teamA && doc.name == "书房组"
              && doc.accountList.map(\.spec) == ["claude:grace@example.com", "claude:carol@example.com", "codex:frank@example.com"]
              && doc.accountList.map(\.alias) == ["team-1", "卡罗尔", "team-1"] && doc.accountList.first?.node == "US-C" && doc.nodes == ["US-A", "US-C"])
        let d = doc.devices
        check("team devices：这台在前；设备编号、管理员起的名字、它在服务器上的名字；别名锁定着", d.count == 6 && d[0].me && d[0].id == "0a1b2c3d4e5f6071"
              && d[1].label == "小王的 MacBook" && d[1].name == "wang-mbp" && d[2].name == nil && doc.aliasLock == true)
        check("team devices：角色（3.6.15 起）：建 Team 的那台、别的管理员、成员；旧版 cca 没有是 nil", d[0].isOwnerDevice && d[1].isAdminDevice
              && d[2].role == "member" && !d[2].isAdminDevice && d[3].role == nil)
        check("team devices：授权：全部 / 只给几个 / 一个都不给", d[0].grantsAll && d[0].has(doc.accountList[2]) && !d[1].grantsAll && d[1].has(doc.accountList[1])
              && !d[1].has(doc.accountList[0]) && d[1].grantCount(of: 3) == 1 && d[0].grantCount(of: 3) == 3 && !d[4].grantsAll && d[4].grantCount(of: 3) == 0)
        check("team devices：状态；不认识的状态不算还在的；移除了的（以前的 cca 还给）不列", d[2].state == .invited && d[2].state.active
              && d[4].state == .revoked && !d[4].state.active && d[4].state.removed && !d[2].state.removed
              && !d[5].state.isKnown && !d[5].state.active && !d[5].state.removed && d[2].expiresEpoch == 1794600000 && d[3].problem?.contains("公钥") == true)
        check("team devices：给人看的那一句", Texts.teamDeviceDetail(d[0], accounts: 3, now: now) == "刚刚来过 · 全部账号 · 配置是最新的"
              && Texts.teamDeviceDetail(d[1], accounts: 3, now: now).hasSuffix("1 个账号 · 还没同步到最新的配置")
              && Texts.teamDeviceDetail(d[2], accounts: 3, now: now) == "邀请还没用（6 天后过期） · 全部账号"
              && Texts.teamDeviceDetail(d[5], accounts: 3, now: now) == "later-state", Texts.teamDeviceDetail(d[0], accounts: 3, now: now))
    }
    check("命令：看设备（展开时自己查的降优先级）、同步配置、退出、邀请、移除、改授权、改名字、改节点、改别名",
          CcaCommand.teamDevices(team: teamA, auto: true).arguments == ["team", "devices", teamA, "--json"] && CcaCommand.teamDevices(team: teamA, auto: true).isBackground
          && !CcaCommand.teamDevices(team: teamA, auto: false).isBackground && CcaCommand.teamSync.arguments == ["team", "sync"]
          && CcaCommand.teamLeave(team: teamA).arguments == ["team", "leave", teamA]
          && CcaCommand.teamInvite(team: teamA, label: "dan-air", accounts: "all").arguments == ["team", "invite", teamA, "dan-air", "--accounts", "all"]
          && CcaCommand.teamRevoke(team: teamA, device: "0a1b").arguments == ["team", "revoke", teamA, "0a1b"]
          && CcaCommand.teamGrant(team: teamA, device: "0a1b", changes: ["none", "+claude:a@example.com"]).arguments
            == ["team", "grant", teamA, "0a1b", "none", "+claude:a@example.com"]
          && CcaCommand.teamRename(team: teamA, device: "0a1b", name: "小王的 MacBook").arguments == ["team", "rename", teamA, "0a1b", "小王的 MacBook"]
          && CcaCommand.teamRename(team: teamA, device: "0a1b", name: "x").gate == CcaCommand.teamRevoke(team: teamA, device: "d").gate
          && !CcaCommand.teamRename(team: teamA, device: "0a1b", name: "x").runsToCompletion
          && CcaCommand.teamMap(team: teamA, tool: .codex, email: "f@example.com", node: "US-A").arguments == ["team", "map", teamA, "codex", "f@example.com", "US-A"]
          && CcaCommand.teamAlias(team: teamA, account: "codex:f@example.com", alias: "设计-1").arguments == ["team", "alias", teamA, "codex:f@example.com", "设计-1"]
          && CcaCommand.teamAlias(team: teamA, account: "codex:f@example.com", alias: "x").runsToCompletion
          && CcaCommand.teamAlias(team: teamA, account: "codex:f@example.com", alias: "x").gate == CcaCommand.routerSync.gate
          && CcaCommand.teamAliasLock(team: teamA, on: true).arguments == ["team", "alias", teamA, "--lock"]
          && CcaCommand.teamAliasLock(team: teamA, on: false).arguments == ["team", "alias", teamA, "--unlock"]
          && CcaCommand.teamAliasLock(team: teamA, on: true).runsToCompletion && CcaCommand.teamAliasLock(team: teamA, on: true).gate == CcaCommand.routerSync.gate)
    check("命令：邀请的输出只经内存里的管道（不是跑完为止的那种）；改账号表的几条跑完为止、和同步路由器排队",
          !CcaCommand.teamInvite(team: teamA, label: "x", accounts: "all").runsToCompletion && CcaCommand.teamSync.runsToCompletion
          && CcaCommand.teamLeave(team: teamA).runsToCompletion && CcaCommand.teamSync.gate == CcaCommand.routerSync.gate
          && CcaCommand.teamGrant(team: teamA, device: "d", changes: ["all"]).gate == CcaCommand.routerSync.gate
          && Handoff.teamJoin.arguments == ["team", "join"] && Handoff.teamJoin.inApp)
    check("提问会话：新建（问名字）、部署（问服务器：和自己的同步服务器同一套表单）、删除（管理员：卸载服务器时问密码、卸不掉时问要不要照样删）；结果码",
          Handoff.teamNew.arguments == ["team", "new"] && Handoff.teamNew.inApp && Handoff.teamNew.needsForms
          && Handoff.teamDeploy(team: teamA).arguments == ["team", "deploy", teamA] && Handoff.teamDeploy(team: teamA).needsForms
          && Handoff.teamDelete(team: teamA).arguments == ["team", "leave", teamA] && Handoff.teamDelete(team: teamA).inApp
          && ResultCode.allKnown.contains(.teamCreated) && ResultCode.allKnown.contains(.teamDeployed) && ResultCode.allKnown.contains(.teamLeft))
    check("hello 的 team_config；旧版 cca 没有", (try? DocumentDecoder.decode(HelloDoc.self, from: Data(#"{"version":1,"kind":"hello","api":1,"team":1,"team_config":1}"#
        .utf8)))?.teamConfig == TeamAPI.config
          && (try? DocumentDecoder.decode(HelloDoc.self, from: Data(#"{"version":1,"kind":"hello","api":1,"team":1}"#.utf8)))?.teamConfig == nil)
    let file = "/Users/me/书房组 Team 配置.json"
    check("命令：导出（保存面板问过覆盖：带 --force；只读配置、写一个文件）、导入（跑完为止、和同步路由器排队）、在终端里修改；结果码、同意项",
          CcaCommand.teamExport(team: teamA, path: file).arguments == ["team", "export", teamA, file, "--force"]
          && !CcaCommand.teamExport(team: teamA, path: file).runsToCompletion
          && CcaCommand.teamExport(team: teamA, path: file).gate == CcaCommand.teamRevoke(team: teamA, device: "d").gate
          && CcaCommand.teamImport(team: teamA, path: file).arguments == ["team", "import", teamA, file]
          && CcaCommand.teamImport(team: teamA, path: file).runsToCompletion && CcaCommand.teamImport(team: teamA, path: file).gate == CcaCommand.routerSync.gate
          && Handoff.teamEdit(team: teamA).arguments == ["team", "edit", teamA] && !Handoff.teamEdit(team: teamA).inApp && Handoff.teamEdit(team: teamA).systemTerminal
          && ResultCode.allKnown.contains(.teamExported) && ResultCode.allKnown.contains(.teamImported) && ConsentItem.allKnown.contains(.teamConfig))
}

/// Team 在界面模型里：筛选（侧栏、菜单栏面板、总览共用），管理员看设备、发邀请（链接只进剪贴板）、改授权
@MainActor func teamModelTests() async {
    section("Team：界面模型")
    guard let hello = fixture("hello.json").map({ String(decoding: $0, as: UTF8.self) }),
          let state = fixture("state.json").map({ String(decoding: $0, as: UTF8.self) }),
          let devices = fixture("team_devices.json").map({ String(decoding: $0, as: UTF8.self) }) else {
        check("Team 的夹具都在", false)
        return
    }
    let link = "cca-team1.demo-link.0000"
    func transport(_ hello: String) -> ScriptedTransport {
        ScriptedTransport { r, _ in
            let a = r.arguments
            if a == ["_gui", "hello"] { return response(0, stdout: hello) }
            if a == ["_gui", "state"] { return response(0, stdout: state) }
            if a.prefix(2) == ["team", "devices"] { return response(0, stdout: devices) }
            if a.prefix(2) == ["team", "invite"] { return response(0, stdout: link + "\n") }
            if a.first == "team" { return response(0) }
            return response(0, stdout: SampleDocs.clientUpdates)
        }
    }
    func teamRuns(_ t: ScriptedTransport) -> [[String]] { t.requests.map(\.arguments).filter { $0.first == "team" } }

    // 旧版 cca（hello 没有 team）：没有 Team 的那一块，不筛选，不调 cca team
    do {
        let t = transport(hello.replacingOccurrences(of: #" "team": 1,"#, with: ""))
        let prefs = MemoryPrefs()
        prefs.set(TeamFilter.team(teamA).stored, forKey: AppModel.Prefs.teamFilter)
        let m = makeModel(t, prefs: prefs)
        await m.start()
        m.joinTeam()
        m.syncTeams()
        try? await Task.sleep(nanoseconds: 100_000_000)
        check("旧版 cca（hello 没有 team）：没有 Team（state 里有也不认）、不筛选（记着的选择不生效）、不调 cca team、不开会话",
              !m.teamAvailable && m.teams.isEmpty && !m.teamFilterAvailable && m.activeTeamFilter == .all
              && m.shownAccounts(.claude).count == m.arrangedAccounts(.claude).count && teamRuns(t).isEmpty && t.started.isEmpty)
        m.shutdown()
    }
    // cca 有 Team、没有导出导入（3.6.13 及以前）：不弹面板、不调 cca team export|import、不开终端
    do {
        let t = transport(hello.replacingOccurrences(of: #" "team_config": 1,"#, with: ""))
        let log = HookLog()
        log.savePath = "/Users/me/a.json"
        log.openPath = "/Users/me/a.json"
        let m = makeModel(t, log: log)
        await m.start()
        m.exportTeamConfig(m.teams[0])
        m.importTeamConfig(m.teams[0])
        m.editTeamConfig(m.teams[0])
        try? await Task.sleep(nanoseconds: 100_000_000)
        check("cca 没有 team_config（旧版）：管理员的 Team 也不导出、不导入、不在终端里改",
              m.teamAvailable && !m.teamConfigAvailable && log.saveNames.isEmpty && log.openAsked == 0
              && !teamRuns(t).contains { ["export", "import", "edit"].contains($0.dropFirst().first ?? "") } && m.toast == nil, "\(teamRuns(t))")
        m.shutdown()
    }

    let t = transport(hello)
    let log = HookLog()
    let prefs = MemoryPrefs()
    let m = makeModel(t, log: log, prefs: prefs)
    await m.start()
    check("cca 有 Team：这台设备在两个 Team 里，能筛选；默认是全部", m.teamAvailable && m.teams.map(\.name) == ["书房组", "设计组"] && m.teamFilterAvailable
          && m.activeTeamFilter == .all && m.teamFilterOptions == [.all, .team(teamA), .team(teamB), .outside])
    let everyone = m.arrangedAccounts(.claude).map(\.key)
    m.teamFilter = .team(teamA)
    check("筛出书房组的：侧栏、菜单栏面板只列它给的账号；排序、写回账号表用的那一份还是全部",
          m.shownAccounts(.claude).map(\.key).sorted() == ["carol@example.com", "grace@example.com", "工作(bob@example.com)"]
          && m.arrangedAccounts(.claude).map(\.key) == everyone
          && m.shownGroups(.claude).count == 1, "\(m.shownAccounts(.claude).map(\.key))")
    check("筛选记进设置", prefs.string(forKey: AppModel.Prefs.teamFilter) == "team:" + teamA)
    let office = m.office(now: 1794042000)
    check("总览也只有这三个账号", office.rooms.flatMap(\.people).map(\.account.key).sorted()
          == ["carol@example.com", "grace@example.com", "工作(bob@example.com)"])
    check("侧栏「总览」那一行的统计不跟着筛选走（别的账号出了问题照样看得到）",
          m.office(now: 1794042000, filtered: false).rooms.flatMap(\.people).count == m.state?.allAccounts.count)
    check("别的工具在这个筛选下没有账号：官方账号那一组留着（标题上有添加按钮）", m.shownGroups(.codex).count == 1 && m.shownGroups(.codex)[0].accounts.isEmpty)
    m.teamFilter = .outside
    check("不在 Team 里的：别的账号都在，第三方 API 的那一组也在",
          m.shownAccounts(.claude).count == everyone.count - 3 && !m.shownAccounts(.claude).contains { $0.team != nil } && m.shownGroups(.claude).count == 2)
    m.teamFilter = .team("0123456789abcdef0123456789abcdef")
    check("选的那个 Team 已经不在了：当作全部", m.activeTeamFilter == .all && m.shownAccounts(.claude).map(\.key) == everyone)
    await m.loadState()
    check("再读到 state 时记着的也改回全部（以后重新加入同一个 Team，筛选不会自己变回去）",
          m.teamFilter == .all && prefs.string(forKey: AppModel.Prefs.teamFilter) == "")

    // 管理员：展开时问一次设备
    let a = m.teams[0]
    check("还没展开：不问服务器", teamRuns(t).isEmpty && m.teamSnapshot(a) == nil)
    m.toggleTeam(a)
    check("展开「设备和账号」：问一次（低优先级）", await waitUntil { m.teamSnapshot(a)?.doc?.devices.count == 6 }
          && teamRuns(t) == [["team", "devices", teamA, "--json"]] && t.requests.last?.background == true)
    m.toggleTeam(m.teams[1])
    try? await Task.sleep(nanoseconds: 80_000_000)
    check("不是管理员的 Team：不问设备", teamRuns(t).count == 1 && m.teamSnapshot(m.teams[1]) == nil)
    guard let doc = m.teamSnapshot(a)?.doc else { return }
    let accounts = doc.accountList
    guard let carol = accounts.first(where: { $0.email == "carol@example.com" }), let grace = accounts.first(where: { $0.email == "grace@example.com" }) else {
        check("夹具里有 carol、grace", false)
        return
    }
    // 发邀请：链接只进剪贴板
    m.teamInvite = TeamInviteDraft(team: teamB, label: "dan-air")
    m.sendTeamInvite(a)
    try? await Task.sleep(nanoseconds: 80_000_000)
    check("正在填的是别的 Team 的邀请：不发", !teamRuns(t).contains { $0.prefix(2) == ["team", "invite"] })
    m.teamInvite = TeamInviteDraft(team: teamA, label: "  dan-air ", all: false, picked: ["codex:frank@example.com", "claude:carol@example.com"])
    m.sendTeamInvite(a)
    _ = await waitUntil { !log.copiedSecrets.isEmpty && !m.teamBusy && m.toast != nil }
    check("发邀请：名字去掉首尾空白、挑的账号排好；链接只经 copySecret（去掉换行），提示、普通的复制里都没有它；填的清掉",
          teamRuns(t).contains(["team", "invite", teamA, "dan-air", "--accounts", "claude:carol@example.com,codex:frank@example.com"])
          && log.copiedSecrets == [link] && log.copied.isEmpty && m.toast?.text.contains(link) == false && m.toast?.text.contains("dan-air") == true
          && m.teamInvite == TeamInviteDraft())
    // 改授权
    m.setTeamGrant(a, doc.devices[0], grace, on: false, all: accounts)
    _ = await waitUntil { !m.teamBusy && teamRuns(t).contains { $0.prefix(2) == ["team", "grant"] } }
    check("全部都给的设备去掉一个账号：改成只给剩下的",
          teamRuns(t).contains(["team", "grant", teamA, "0a1b2c3d4e5f6071", "none", "+claude:carol@example.com", "+codex:frank@example.com"]))
    m.setTeamGrant(a, doc.devices[1], grace, on: true, all: accounts)
    _ = await waitUntil { !m.teamBusy && teamRuns(t).contains(["team", "grant", teamA, "1b2c3d4e5f607182", "+claude:grace@example.com"]) }
    check("只给几个的设备加一个", teamRuns(t).contains(["team", "grant", teamA, "1b2c3d4e5f607182", "+claude:grace@example.com"]))
    m.mapTeamAccount(a, carol, node: "US-C")
    try? await Task.sleep(nanoseconds: 60_000_000)
    check("节点没变：不发命令", !teamRuns(t).contains { $0.prefix(2) == ["team", "map"] })
    m.mapTeamAccount(a, carol, node: "US-A")
    check("改节点", await waitUntil { !m.teamBusy && teamRuns(t).contains(["team", "map", teamA, "claude", "carol@example.com", "US-A"]) })
    // 改名字：打开输入框时填着现在的名字；名字没变不发命令；改成了收起输入框
    let dev0 = doc.devices[0]
    m.startTeamRename(a, dev0)
    check("「改名…」：输入框里先填着现在的名字", m.teamRename == TeamRenameDraft(team: teamA, device: dev0.id, name: dev0.label))
    m.renameTeamDevice(a, dev0)
    try? await Task.sleep(nanoseconds: 60_000_000)
    check("名字没变：不发命令，输入框收起", !teamRuns(t).contains { $0.prefix(2) == ["team", "rename"] } && m.teamRename == nil)
    m.startTeamRename(a, dev0)
    m.teamRename?.name = "  书房的 Mac mini "
    m.renameTeamDevice(m.teams[1], dev0)
    try? await Task.sleep(nanoseconds: 60_000_000)
    check("输入框是别的 Team 的设备的：不发", !teamRuns(t).contains { $0.prefix(2) == ["team", "rename"] } && m.teamRename != nil)
    m.renameTeamDevice(a, dev0)
    check("改名字：去掉首尾空白，按设备编号改；改成了收起输入框",
          await waitUntil { !m.teamBusy && m.teamRename == nil } && teamRuns(t).contains(["team", "rename", teamA, dev0.id, "书房的 Mac mini"]))
    // 改别名：输入框开在点的那一行（同一个邮箱别的工具的那一行不开）；别名没变不发命令；改成了收起
    let acct = accounts[0]
    m.startTeamAlias(a, acct)
    check("「改别名…」：输入框里先填着现在的别名，开在这一行", m.teamAlias == TeamAliasDraft(team: teamA, spec: acct.spec, email: acct.email, alias: acct.alias ?? ""))
    m.teamAlias?.alias = acct.alias ?? ""
    m.setTeamAlias(a, acct)
    try? await Task.sleep(nanoseconds: 60_000_000)
    check("别名没变：不发命令，输入框收起", !teamRuns(t).contains { $0.prefix(2) == ["team", "alias"] } && m.teamAlias == nil)
    m.startTeamAlias(a, acct)
    m.teamAlias?.alias = " 书房-1 "
    m.setTeamAlias(a, acct)
    check("改别名：去掉首尾空白，按 工具:邮箱 改（只改这个工具里的）；改成了收起输入框",
          await waitUntil { !m.teamBusy && m.teamAlias == nil } && teamRuns(t).contains(["team", "alias", teamA, acct.spec, "书房-1"]))
    // 导出、导入（hello.team_config）：先弹面板，取消了不发命令；不是管理员的 Team 都不做
    check("cca 有 team_config", m.teamConfigAvailable)
    m.exportTeamConfig(a)
    _ = await waitUntil { !log.saveNames.isEmpty }
    try? await Task.sleep(nanoseconds: 60_000_000)
    check("导出：保存面板里先填 Team 的名字；取消了不发命令", log.saveNames == ["书房组 Team 配置.json"] && !teamRuns(t).contains { $0.prefix(2) == ["team", "export"] })
    log.savePath = "/Users/me/书房组 Team 配置.json"
    m.exportTeamConfig(a)
    check("导出：选好了文件：cca team export … --force", await waitUntil { !m.teamBusy && teamRuns(t).contains(["team", "export", teamA, "/Users/me/书房组 Team 配置.json", "--force"]) })
    m.importTeamConfig(a)
    _ = await waitUntil { log.openAsked == 1 }
    try? await Task.sleep(nanoseconds: 60_000_000)
    check("导入：取消了不发命令", log.openAsked == 1 && !teamRuns(t).contains { $0.prefix(2) == ["team", "import"] })
    log.openPath = "/Users/me/改过的.json"
    m.importTeamConfig(a)
    check("导入：选好了文件：cca team import（同意由 cca 问）", await waitUntil { !m.teamBusy && teamRuns(t).contains(["team", "import", teamA, "/Users/me/改过的.json"]) })
    let runs = teamRuns(t).count
    m.exportTeamConfig(m.teams[1])
    m.importTeamConfig(m.teams[1])
    m.editTeamConfig(m.teams[1])
    try? await Task.sleep(nanoseconds: 80_000_000)
    check("不是管理员的 Team：不导出、不导入、不改", teamRuns(t).count == runs && log.saveNames.count == 2 && log.openAsked == 2 && t.started.isEmpty)
    m.editTeamConfig(a)
    check("在终端里修改：不开提问会话，交给终端（测试里没有 cca 的路径：说找不到 cca）", t.started.isEmpty && m.toast?.text == "找不到 cca", "\(m.toast?.text ?? "")")
    m.joinTeam()
    check("「粘贴邀请链接…」：开提问会话 cca team join", await waitUntil { !t.started.isEmpty } && t.started == [["team", "join"]], "\(t.started)")
    m.shutdown()
    // 新建、部署各开一个提问会话；不是管理员的 Team 不部署
    for (label, want, act) in [("新建 Team…", ["team", "new"], { (m: AppModel) in m.newTeam() }),
                               ("部署到我的服务器…", ["team", "deploy", teamA], { (m: AppModel) in m.deployTeam(m.teams[0]) })] {
        let t2 = transport(hello)
        let m2 = makeModel(t2)
        await m2.start()
        m2.deployTeam(m2.teams[1])
        act(m2)
        check("\(label)：开提问会话 cca \(want.joined(separator: " "))（成员的 Team 不部署）", await waitUntil { !t2.started.isEmpty } && t2.started == [want], "\(t2.started)")
        m2.shutdown()
    }
    // 删掉（管理员）开提问会话：卸载服务器时要问密码、卸不掉时问要不要照样删。成员退出照旧是一条命令（先同意再重跑）
    let t3 = transport(hello)
    let m3 = makeModel(t3)
    await m3.start()
    m3.leaveTeam(m3.teams[1])
    check("退出这个 Team…（成员）：一条命令 cca team leave，不开会话",
          await waitUntil { teamRuns(t3).contains(["team", "leave", teamB]) && !m3.teamBusy } && t3.started.isEmpty, "\(teamRuns(t3)) \(t3.started)")
    m3.leaveTeam(m3.teams[0])
    check("删掉这个 Team…（管理员）：开提问会话 cca team leave", await waitUntil { !t3.started.isEmpty } && t3.started == [["team", "leave", teamA]]
          && !teamRuns(t3).contains(["team", "leave", teamA]), "\(t3.started)")
    m3.shutdown()
}
