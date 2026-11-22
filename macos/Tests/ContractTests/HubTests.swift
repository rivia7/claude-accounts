import Foundation

/// 同步服务器：维护者的、各个 Team 的（PROTOCOL 2.2 的 `config.hubs`、账号的 `hub`，3.4、3.5 的 `hubs`）。3.5.3 以前还有自己的同步服务器
/// （`own`、`cca hub`）：旧版 cca 给的当作不认识。只在 --full（手写的 Tests/Fixtures）里跑
func hubFixtureTests() {
    section("同步服务器")
    if let h = loadFixture(HelloDoc.self, "hello.json") {
        check("hello：有 Team（自己的同步服务器去掉了，没有 hub 这一项）", h.team == TeamAPI.version)
    }
    if let s = loadFixture(StateDoc.self, "state.json") {
        let hubs = s.config?.hubs ?? []
        check("state：config.hubs 里维护者的那一台，不认识的 kind 去掉了", hubs.map(\.kind) == [.maintainer])
        check("state：维护者的服务器经它同步 1 个账号，这台在它上面叫 studio-mac，同步正常", hubs.first?.name == "studio-mac"
              && hubs.first?.accounts == 1 && hubs.first?.state == .ok && hubs.first?.state.failing == false && hubs.first?.team == nil)
        let bob = s.account(AccountRef(tool: .claude, key: "工作(bob@example.com)"))
        let carol = s.account(AccountRef(tool: .claude, key: "carol@example.com"))
        let erin = s.account(AccountRef(tool: .claude, key: "erin@example.com"))
        check("state：bob 经维护者的服务器同步，折合金额也是", bob?.hub == .maintainer && bob?.cost?.sync?.hub == .maintainer)
        check("state：carol 不经服务器同步", carol?.hub == nil)
        check("state：只在这台设备上的折合金额（local_only）不说经哪台", erin?.cost?.sync?.state == .localOnly && erin?.cost?.sync?.hub == nil)
    }
    if let s = loadFixture(StateDoc.self, "state_cost_sync.json") {
        let alice = s.account(AccountRef(tool: .claude, key: "alice@example.com"))
        check("旧版 cca 的 state（没有 hub、config.hubs）：有「设备」一节的账号当作经维护者的服务器，折合金额也是；config.hubs 是空的",
              alice?.hub == .maintainer && alice?.can.devices == true && alice?.cost?.sync?.hub == .maintainer && s.config?.hubs.isEmpty == true)
    }
    if let d = fixture("state.json"), let text = String(data: d, encoding: .utf8) {
        // 以后的 cca 给了不认识的 hub：当作 null（不经服务器同步），有「设备」一节也不当作维护者的
        let later = Data(text.replacingOccurrences(of: #""hub": "maintainer""#, with: #""hub": "later-kind""#).utf8)
        let s = try? DocumentDecoder.decode(StateDoc.self, from: later)
        let bob = s?.account(AccountRef(tool: .claude, key: "工作(bob@example.com)"))
        check("state：不认识的 hub 当作 null（有「设备」一节也一样），折合金额也是", bob != nil && bob?.can.devices == true && bob?.hub == nil
              && bob?.cost?.sync?.hub == nil)
        // 3.5.2 的 cca：config.hubs 里有自己的同步服务器、账号走它——App 不认识 own 了，当作没有
        let own = Data(text.replacingOccurrences(of: #""kind": "later-kind""#, with: #""kind": "own""#)
            .replacingOccurrences(of: #""hub": "maintainer""#, with: #""hub": "own""#).utf8)
        let o = try? DocumentDecoder.decode(StateDoc.self, from: own)
        let ob = o?.account(AccountRef(tool: .claude, key: "工作(bob@example.com)"))
        check("旧版 cca 的自己的同步服务器（own）：config.hubs 里去掉，走它的账号当作不经服务器同步", o?.config?.hubs.map(\.kind) == [.maintainer]
              && ob != nil && ob?.hub == nil && ob?.cost?.sync?.hub == nil)
    }
    let teamHub = try? JSONDecoder().decode(HubEntry.self, from: Data(#"""
        {"kind":"team","url":"https://team.example.com:18445","host":"team.example.com:18445","state":"error","error":"连不上","checked_epoch":null,
         "accounts":2,"name":"studio-mac","members":null,"joined_epoch":null,"can_invite":false,"pinned":null,
         "team":"5d0c2a9be17f4c3e8a6b1d2f3e4c5a6b","team_name":"书房组"}
        """#.utf8))
    check("config.hubs 里 Team 的服务器：是哪个 Team、叫什么", teamHub?.kind == .team && teamHub?.team == "5d0c2a9be17f4c3e8a6b1d2f3e4c5a6b"
          && teamHub?.teamName == "书房组" && teamHub?.state.failing == true && teamHub?.error == "连不上" && teamHub?.accounts == 2)
    if let doc = loadFixture(DevicesDoc.self, "devices.json"), let a = doc.accounts.first {
        let h = doc.hub(for: a)
        check("旧版 cca 的 devices --json（没有 hubs、账号的 hub）：当作维护者的服务器，设置和名字用顶层的",
              doc.hubs.isEmpty && a.hub == nil && h.kind == .maintainer && h.me == doc.me && h.idleAfter == doc.idleAfter && h.offlineAfter == doc.offlineAfter)
    }
    let tid = "5d0c2a9be17f4c3e8a6b1d2f3e4c5a6b"
    let two = try? DocumentDecoder.decode(DevicesDoc.self, from: Data(#"""
        {"version":1,"kind":"devices","generated_epoch":1,"tool":"claude","me":"m-a","idle_after":600,"offline_after":150,"reporting_error":null,
         "hubs":[{"kind":"maintainer","me":"m-a","idle_after":600,"offline_after":150,"reporting_error":null},
                 {"kind":"team","team":"\#(tid)","me":"t-a","idle_after":300,"offline_after":90,"reporting_error":"连不上"}],
         "accounts":[{"key":"a(a@example.com)","hub":"maintainer","devices":[]},{"key":"b(b@example.com)","hub":"team","team":"\#(tid)","devices":[]}]}
        """#.utf8))
    if let two, two.accounts.count == 2 {
        let t = two.hub(for: two.accounts[1])
        check("devices --json 两台服务器：各账号按自己的 hub（Team 的照编号）用那一台的名字和设置", two.hub(for: two.accounts[0]).me == "m-a"
              && t.kind == .team && t.me == "t-a" && t.idleAfter == 300 && t.offlineAfter == 90 && t.reportingError == "连不上")
    } else {
        check("devices --json 两台服务器：解码了", false)
    }
    if let doc = loadFixture(CostSyncDoc.self, "cost_sync.json") {
        check("旧版 cca 的 cost sync --json（没有 hubs）：hubs 是空的", doc.sync?.hubs.isEmpty == true && doc.sync?.kind == nil)
    }
    let sync = try? DocumentDecoder.decode(CostSyncDoc.self, from: Data(#"""
        {"version":1,"kind":"cost_sync","generated_epoch":1,"scan":{"state":"ready","updated_epoch":1,"files":0,"read_bytes":0,"records":0,"first":false},
         "sync":{"state":"error","message":"连不上","checked_epoch":null,"hubs":[{"kind":"maintainer","state":"ok","message":null,"checked_epoch":5},
                                                                          {"kind":"team","team":"\#(tid)","state":"error","message":"连不上","checked_epoch":null}]}}
        """#.utf8))
    check("cost sync --json 两台服务器：顶层是最要紧的那一台，hubs 各是各的", sync?.sync?.state == .error && sync?.sync?.hubs.map(\.kind) == [.maintainer, .team]
          && sync?.sync?.hubs.first?.checkedEpoch == 5 && sync?.sync?.hubs.last?.message == "连不上")
    check("Team 部署时的代码认识：密码错了三次（hub_ssh）、交给终端（hub_deploy）、装好了但地址是内网的（team_lan）", FailCode.hubSSH.isKnown
          && TerminalCode.hubDeploy.isKnown && ResultCode.teamDeployed.isKnown && FailCode.teamLan.isKnown)
    check("自己的同步服务器的结果、失败、同意的代码不再认识（旧版 cca 给了照 message 说）",
          [ResultCode(rawValue: "hub_joined"), ResultCode(rawValue: "hub_deployed"), ResultCode(rawValue: "hub_hosted")].allSatisfy { !$0.isKnown }
          && [FailCode(rawValue: "hub_link"), FailCode(rawValue: "hub_unavailable")].allSatisfy { !$0.isKnown }
          && [ConsentItem(rawValue: "hub_remove"), ConsentItem(rawValue: "hub_leave"), ConsentItem(rawValue: "hub_relink")].allSatisfy { !$0.isKnown })
    let terminal = EventLog.parseLine(Data(#"{"v":1,"ev":"terminal","code":"hub_deploy","argv":["team","deploy","5d0c2a9b","root@203.0.113.7","-p","22"],"display":"cca team deploy 5d0c2a9b root@203.0.113.7 -p 22","message":"服务器只认密钥：在终端里部署"}"#.utf8))?.terminal
    let h = terminal.map(Handoff.init(event:))
    check("服务器只认密钥时交给终端：照 cca 给的整条命令（Team、服务器、端口）", terminal?.code == .hubDeploy
          && h?.arguments == ["team", "deploy", "5d0c2a9b", "root@203.0.113.7", "-p", "22"])
    let homes = ["/Users/me/.cca"]
    let cases: [(String, CcaWatcher.Trigger?)] = [
        ("/Users/me/.cca/cost/team.\(tid).remote.json", .state), ("/Users/me/.cca/cost/team.\(tid).sync.json", .state),
        ("/Users/me/.cca/cost/team.\(tid).lock", nil), ("/Users/me/.cca/cost/team.\(tid.uppercased()).remote.json", nil),
        ("/Users/me/.cca/cost/team.0123.remote.json", nil), ("/Users/me/.cca/cost/team.\(tid).remote.json.tmp", nil),
        ("/Users/me/.cca/presence/status.json", .state), ("/Users/me/.cca/presence/team.\(tid).status.json", .state),
        ("/Users/me/.cca/presence/team.\(tid).keys.json", nil), ("/Users/me/.cca/presence/keys.json", nil), ("/Users/me/.cca/presence/agent.json", nil),
        ("/Users/me/.cca/cost/own.remote.json", nil), ("/Users/me/.cca/hub/hub.json", nil), ("/Users/me/.cca/presence/own.status.json", nil),
    ]
    let wrong = cases.filter { CcaWatcher.classify(path: $0.0, homes: homes) != $0.1 }.map(\.0)
    check("看着设备统计报没报上去、经各台服务器同步来的折合金额（各个 Team 的照编号认；别的、自己的同步服务器留下的不算）", wrong.isEmpty,
          wrong.joined(separator: " "))
}

/// 「共享与同步」页：维护者共享的开关什么时候显示，侧栏那一行说什么（照 cca 给的 config.policy、config.teams）
@MainActor func sharingPageTests() async {
    section("共享与同步：Team 和维护者的共享")
    guard let hello = fixture("hello.json").map({ String(decoding: $0, as: UTF8.self) }) else {
        check("夹具", false)
        return
    }
    let noTeam = hello.replacingOccurrences(of: #" "team": 1,"#, with: "")
    let cases: [(String, String, [String: Any], Bool)] = [
        ("有 Team 的 cca、共享关着、没收起账号：不显示维护者的共享（经 Team 拿账号）", hello,
         ["configured": false, "repo": NSNull(), "next_check_epoch": NSNull(), "state": "off", "accounts": 0, "parked": 0], false),
        ("共享关着、关掉时收起了账号：显示（打开就原样加回来）", hello,
         ["configured": false, "repo": NSNull(), "next_check_epoch": NSNull(), "state": "off", "accounts": 0, "parked": 2], true),
        ("共享开着：显示", hello, ["configured": true, "repo": "example/cca-policy", "next_check_epoch": NSNull(), "state": "on", "accounts": 2, "parked": 0], true),
        ("还在等维护者：显示", hello, ["configured": true, "repo": "example/cca-policy", "next_check_epoch": NSNull(), "state": "pending", "accounts": 0, "parked": 0], true),
        ("没有 Team 的旧版 cca：照旧显示", noTeam,
         ["configured": false, "repo": NSNull(), "next_check_epoch": NSNull(), "state": "off", "accounts": 0, "parked": 0], true),
    ]
    for (label, h, policy, want) in cases {
        let state = stateWithPolicy(policy)
        let t = ScriptedTransport { r, _ in
            if r.arguments == ["_gui", "hello"] { return response(0, stdout: h) }
            if r.arguments == ["_gui", "state"] { return response(0, stdout: state) }
            return response(0, stdout: SampleDocs.clientUpdates)
        }
        let m = makeModel(t)
        await m.connect()
        check(label, m.sharingAvailable && m.policyCardShown == want, "\(m.policyCardShown)")
        m.shutdown()
    }
    check("侧栏那一行：在一个 Team 里说名字，几个说几个，维护者的共享还开着时带一句",
          Texts.sharingSubtitle(policyOn: false, teams: ["书房组"]) == "在「书房组」里"
          && Texts.sharingSubtitle(policyOn: true, teams: ["书房组"]) == "「书房组」 · 共享开着"
          && Texts.sharingSubtitle(policyOn: false, teams: ["书房组", "设计组"]) == "在 2 个 Team 里"
          && Texts.sharingSubtitle(policyOn: false, teams: [""]) == "在「Team」里")
    check("侧栏那一行：不在 Team 里时说维护者的共享开没开；没有 Team 的旧版 cca 照旧",
          Texts.sharingSubtitle(policyOn: false, teams: []) == "还没加入 Team" && Texts.sharingSubtitle(policyOn: true, teams: []) == "维护者的共享开着"
          && Texts.sharingSubtitle(policyOn: false, teams: nil) == "维护者的共享关着" && Texts.sharingSubtitle(policyOn: true, teams: nil) == "维护者的共享开着")
}
