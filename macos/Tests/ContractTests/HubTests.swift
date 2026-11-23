import Foundation

/// 同步服务器：各个 Team 的（PROTOCOL 2.2 的 `config.hubs`、账号的 `hub`，3.4、3.5 的 `hubs`）。3.5.3 以前还有自己的同步服务器
/// （`own`、`cca hub`），3.6.0 以前还有维护者的服务器（`maintainer`）：旧版 cca 给的都当作不认识。只在 --full（手写的 Tests/Fixtures）里跑
func hubFixtureTests() {
    section("同步服务器")
    let tid = "5d0c2a9be17f4c3e8a6b1d2f3e4c5a6b", other = "0123456789abcdef0123456789abcdef"
    if let h = loadFixture(HelloDoc.self, "hello.json") {
        check("hello：有 Team（自己的同步服务器去掉了，没有 hub 这一项）", h.team == TeamAPI.version)
    }
    if let s = loadFixture(StateDoc.self, "state.json") {
        let hubs = s.config?.hubs ?? []
        check("state：config.hubs 里是书房组的那一台（编号、名字），不认识的 kind 去掉了", hubs.map(\.kind) == [.team]
              && hubs.first?.team == tid && hubs.first?.teamName == "书房组")
        check("state：书房组的服务器经它同步 1 个账号，这台在它上面叫 studio-mac，同步正常", hubs.first?.name == "studio-mac"
              && hubs.first?.accounts == 1 && hubs.first?.state == .ok && hubs.first?.state.failing == false)
        let bob = s.account(AccountRef(tool: .claude, key: "工作(bob@example.com)"))
        let carol = s.account(AccountRef(tool: .claude, key: "carol@example.com"))
        let erin = s.account(AccountRef(tool: .claude, key: "erin@example.com"))
        check("state：bob 是书房组给的，经它的服务器同步，折合金额也是", bob?.hub == .team && bob?.team == tid && bob?.can.devices == true
              && bob?.cost?.sync?.hub == .team)
        check("state：carol 不经服务器同步", carol?.hub == nil)
        check("state：只在这台设备上的折合金额（local_only）不说经哪台", erin?.cost?.sync?.state == .localOnly && erin?.cost?.sync?.hub == nil)
    }
    if let s = loadFixture(StateDoc.self, "state_cost_sync.json") {
        let alice = s.account(AccountRef(tool: .claude, key: "alice@example.com"))
        check("旧版 cca 的 state（没有 hub、config.hubs）：有「设备」一节也不当作经哪台服务器同步（那时只有维护者的），折合金额也是；config.hubs 是空的",
              alice != nil && alice?.hub == nil && alice?.can.devices == true && alice?.cost?.sync?.hub == nil && s.config?.hubs.isEmpty == true)
    }
    if let d = fixture("state.json"), let text = String(data: d, encoding: .utf8) {
        let teamEntry = #"{"kind": "team", "url": "https://team.example.com:18445""#
        func decoded(_ pairs: [(String, String)]) -> (StateDoc?, Account?, Bool) {
            var t = text, hit = true
            for (a, b) in pairs {
                hit = hit && t.contains(a)
                t = t.replacingOccurrences(of: a, with: b)
            }
            let s = try? DocumentDecoder.decode(StateDoc.self, from: Data(t.utf8))
            return (s, s?.account(AccountRef(tool: .claude, key: "工作(bob@example.com)")), hit)
        }
        // 以后的 cca 给了不认识的 hub：当作 null（不经服务器同步），有「设备」一节也不当作哪个 Team 的
        let (_, later, laterHit) = decoded([(#""hub": "team""#, #""hub": "later-kind""#)])
        check("state：不认识的 hub 当作 null（有「设备」一节也一样），折合金额也是", laterHit && later != nil && later?.can.devices == true && later?.hub == nil
              && later?.cost?.sync?.hub == nil)
        // 3.5.5 的 cca：config.hubs 里有维护者的服务器、账号走它（hub 是 maintainer）——App 不认识了，当作没有
        let (m, mb, mHit) = decoded([(teamEntry, #"{"kind": "maintainer", "url": "https://team.example.com:18445""#),
                                     (#""hub": "team""#, #""hub": "maintainer""#)])
        check("3.6.0 以前的维护者的服务器（maintainer）：config.hubs 里去掉，走它的账号当作不经服务器同步，折合金额也是", mHit
              && m?.config?.hubs.isEmpty == true && mb != nil && mb?.hub == nil && mb?.cost?.sync?.hub == nil && mb?.can.devices == true)
        // 3.5.2 的 cca：config.hubs 里有自己的同步服务器、账号走它——App 不认识 own 了，当作没有
        let (o, ob, oHit) = decoded([(#""kind": "later-kind""#, #""kind": "own""#), (#""hub": "team""#, #""hub": "own""#)])
        check("旧版 cca 的自己的同步服务器（own）：config.hubs 里去掉，走它的账号当作不经服务器同步", oHit && o?.config?.hubs.map(\.kind) == [.team]
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
        check("devices --json：账号经书房组的服务器问，设置和名字用 hubs 里它那一台的（照编号认）", a.hub == .team && a.team == tid && doc.hubs.count == 1
              && h.kind == .team && h.team == tid && h.me == "studio-mac" && h.idleAfter == 600 && h.offlineAfter == 150)
    }
    let old = try? DocumentDecoder.decode(DevicesDoc.self, from: Data(#"""
        {"version":1,"kind":"devices","generated_epoch":1,"tool":"claude","me":"m-a","idle_after":600,"offline_after":150,"reporting_error":null,
         "accounts":[{"key":"a(a@example.com)","devices":[]}]}
        """#.utf8))
    if let old, let a = old.accounts.first {
        let h = old.hub(for: a)
        check("旧版 cca 的 devices --json（没有 hubs、账号的 hub）：设置和名字用顶层的，不当成哪个 Team 的", old.hubs.isEmpty && a.hub == nil
              && !h.kind.isKnown && h.team == nil && h.me == "m-a" && h.idleAfter == 600 && h.offlineAfter == 150)
    } else {
        check("旧版 cca 的 devices --json：解码了", false)
    }
    let two = try? DocumentDecoder.decode(DevicesDoc.self, from: Data(#"""
        {"version":1,"kind":"devices","generated_epoch":1,"tool":"claude","me":"m-a","idle_after":600,"offline_after":150,"reporting_error":null,
         "hubs":[{"kind":"team","team":"\#(other)","me":"m-a","idle_after":600,"offline_after":150,"reporting_error":null},
                 {"kind":"team","team":"\#(tid)","me":"t-a","idle_after":300,"offline_after":90,"reporting_error":"连不上"}],
         "accounts":[{"key":"a(a@example.com)","hub":"team","team":"\#(other)","devices":[]},{"key":"b(b@example.com)","hub":"team","team":"\#(tid)","devices":[]}]}
        """#.utf8))
    if let two, two.accounts.count == 2 {
        let t = two.hub(for: two.accounts[1])
        check("devices --json 两个 Team 的服务器：各账号照编号用那一台的名字和设置", two.hub(for: two.accounts[0]).me == "m-a"
              && two.hub(for: two.accounts[0]).team == other && t.kind == .team && t.team == tid && t.me == "t-a" && t.idleAfter == 300
              && t.offlineAfter == 90 && t.reportingError == "连不上")
    } else {
        check("devices --json 两个 Team 的服务器：解码了", false)
    }
    if let doc = loadFixture(CostSyncDoc.self, "cost_sync.json") {
        check("cost sync --json：hubs 里是书房组的那一台，顶层是它的", doc.sync?.hubs.map(\.kind) == [.team] && doc.sync?.state == .ok
              && doc.sync?.hubs.first?.state == .ok && doc.sync?.kind == nil)
    }
    let oldSync = try? DocumentDecoder.decode(CostSyncDoc.self, from: Data(#"""
        {"version":1,"kind":"cost_sync","generated_epoch":1,"scan":{"state":"ready","updated_epoch":1,"files":0,"read_bytes":0,"records":0,"first":false},
         "sync":{"state":"ok","message":null,"checked_epoch":1}}
        """#.utf8))
    check("旧版 cca 的 cost sync --json（没有 hubs）：hubs 是空的", oldSync?.sync?.hubs.isEmpty == true && oldSync?.sync?.kind == nil)
    let sync = try? DocumentDecoder.decode(CostSyncDoc.self, from: Data(#"""
        {"version":1,"kind":"cost_sync","generated_epoch":1,"scan":{"state":"ready","updated_epoch":1,"files":0,"read_bytes":0,"records":0,"first":false},
         "sync":{"state":"error","message":"连不上","checked_epoch":null,"hubs":[{"kind":"team","team":"\#(other)","state":"ok","message":null,"checked_epoch":5},
                                                                          {"kind":"team","team":"\#(tid)","state":"error","message":"连不上","checked_epoch":null}]}}
        """#.utf8))
    check("cost sync --json 两个 Team 的服务器：顶层是最要紧的那一台，hubs 各是各的", sync?.sync?.state == .error && sync?.sync?.hubs.map(\.kind) == [.team, .team]
          && sync?.sync?.hubs.first?.checkedEpoch == 5 && sync?.sync?.hubs.last?.message == "连不上")
    check("Team 部署时的代码认识：密码错了三次（hub_ssh）、交给终端（hub_deploy）、装好了但地址是内网的（team_lan）", FailCode.hubSSH.isKnown
          && TerminalCode.hubDeploy.isKnown && ResultCode.teamDeployed.isKnown && FailCode.teamLan.isKnown)
    check("自己的同步服务器的结果、失败、同意的代码不再认识（旧版 cca 给了照 message 说）",
          [ResultCode(rawValue: "hub_joined"), ResultCode(rawValue: "hub_deployed"), ResultCode(rawValue: "hub_hosted")].allSatisfy { !$0.isKnown }
          && [FailCode(rawValue: "hub_link"), FailCode(rawValue: "hub_unavailable")].allSatisfy { !$0.isKnown }
          && [ConsentItem(rawValue: "hub_remove"), ConsentItem(rawValue: "hub_leave"), ConsentItem(rawValue: "hub_relink")].allSatisfy { !$0.isKnown })
    check("维护者的共享的结果、失败、同意的代码不再认识（3.6.0 以前的 cca 给了照 message 说）",
          [ResultCode(rawValue: "policy_on"), ResultCode(rawValue: "policy_off")].allSatisfy { !$0.isKnown }
          && [FailCode(rawValue: "policy_unavailable"), FailCode(rawValue: "policy_off_busy")].allSatisfy { !$0.isKnown }
          && !ConsentItem(rawValue: "policy_off").isKnown && !HubKind(rawValue: "maintainer").isKnown
          && !CostSyncState(rawValue: "old_server").isKnown && CostSyncState(rawValue: "old_server").shown == .error)
    let terminal = EventLog.parseLine(Data(#"{"v":1,"ev":"terminal","code":"hub_deploy","argv":["team","deploy","5d0c2a9b","root@203.0.113.7","-p","22"],"display":"cca team deploy 5d0c2a9b root@203.0.113.7 -p 22","message":"服务器只认密钥：在终端里部署"}"#.utf8))?.terminal
    let h = terminal.map(Handoff.init(event:))
    check("服务器只认密钥时交给终端：照 cca 给的整条命令（Team、服务器、端口）", terminal?.code == .hubDeploy
          && h?.arguments == ["team", "deploy", "5d0c2a9b", "root@203.0.113.7", "-p", "22"])
    let homes = ["/Users/me/.cca"]
    let cases: [(String, CcaWatcher.Trigger?)] = [
        ("/Users/me/.cca/cost/team.\(tid).remote.json", .state), ("/Users/me/.cca/cost/team.\(tid).sync.json", .state),
        ("/Users/me/.cca/cost/team.\(tid).lock", nil), ("/Users/me/.cca/cost/team.\(tid.uppercased()).remote.json", nil),
        ("/Users/me/.cca/cost/team.0123.remote.json", nil), ("/Users/me/.cca/cost/team.\(tid).remote.json.tmp", nil),
        ("/Users/me/.cca/presence/team.\(tid).status.json", .state),
        ("/Users/me/.cca/presence/team.\(tid).keys.json", nil), ("/Users/me/.cca/presence/keys.json", nil), ("/Users/me/.cca/presence/agent.json", nil),
        ("/Users/me/.cca/cost/own.remote.json", nil), ("/Users/me/.cca/hub/hub.json", nil), ("/Users/me/.cca/presence/own.status.json", nil),
        // 维护者的服务器（3.6.0 去掉了）留下的：不带 Team 编号的那几份
        ("/Users/me/.cca/presence/status.json", nil), ("/Users/me/.cca/cost/remote.json", nil), ("/Users/me/.cca/cost/sync.json", nil),
    ]
    let wrong = cases.filter { CcaWatcher.classify(path: $0.0, homes: homes) != $0.1 }.map(\.0)
    check("看着设备统计报没报上去、经各个 Team 的服务器同步来的折合金额（照编号认；别的、维护者的服务器和自己的同步服务器留下的不算）", wrong.isEmpty,
          wrong.joined(separator: " "))
}

/// 「共享与同步」页：只在 cca 有 Team（`hello.team`）时有，侧栏那一行说在哪几个 Team 里。3.6.0 以前的 cca 给的 `config.policy`
/// （维护者的共享开着、在等维护者……）不管：3.6.0 起它固定是关着的那一份，App 不读
@MainActor func sharingPageTests() async {
    section("共享与同步：Team")
    guard let hello = fixture("hello.json").map({ String(decoding: $0, as: UTF8.self) }) else {
        check("夹具", false)
        return
    }
    let noTeam = hello.replacingOccurrences(of: #" "team": 1,"#, with: "")
    let off: [String: Any] = ["configured": false, "repo": NSNull(), "next_check_epoch": NSNull(), "state": "off", "accounts": 0, "parked": 0]
    let on: [String: Any] = ["configured": true, "repo": "example/cca-policy", "next_check_epoch": NSNull(), "state": "on", "accounts": 2, "parked": 0]
    let pending: [String: Any] = ["configured": true, "repo": "example/cca-policy", "next_check_epoch": NSNull(), "state": "pending", "accounts": 0, "parked": 0]
    let cases: [(String, String, [String: Any]?, Bool)] = [
        ("有 Team 的 cca（config.policy 是固定的关着）：有这一页", hello, off, true),
        ("3.6.0 以前的 cca、维护者的共享开着：照样只看 Team，有这一页", hello, on, true),
        ("3.6.0 以前的 cca、还在等维护者：一样", hello, pending, true),
        ("没有 config.policy：一样", hello, nil, true),
        ("没有 Team 的旧版 cca：没有这一页（维护者的共享开着也没有）", noTeam, on, false),
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
        m.selection = .sharing
        await m.loadState()
        check(label, m.teamAvailable == want && m.state != nil && (m.selection == .sharing) == want
              && !t.requests.contains { $0.arguments.first == "policy" }, "\(String(describing: m.selection))")
        m.shutdown()
    }
    check("侧栏那一行：在一个 Team 里说名字，几个说几个，没名字的说 Team",
          Texts.sharingSubtitle(teams: ["书房组"]) == "在「书房组」里" && Texts.sharingSubtitle(teams: ["书房组", "设计组"]) == "在 2 个 Team 里"
          && Texts.sharingSubtitle(teams: [""]) == "在「Team」里")
    check("侧栏那一行：不在 Team 里时说还没加入", Texts.sharingSubtitle(teams: []) == "还没加入 Team")
}
