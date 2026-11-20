import Foundation

/// 自己的同步服务器（PROTOCOL 2.2 的 `config.hubs`、账号的 `hub`，3.4、3.5 的 `hubs`，4.13 的 `cca hub`）：只在 --full（手写的 Tests/Fixtures）里跑
func hubFixtureTests() {
    section("自己的同步服务器")
    if let h = loadFixture(HelloDoc.self, "hello.json") {
        check("hello：hub（cca 有自己的同步服务器）", h.hub == 1 && h.hub == HubAPI.version)
    }
    if let h = loadFixture(HelloDoc.self, "hello_nocodex_notinstalled.json") {
        check("hello：旧版 cca 没有 hub", h.hub == nil)
    }
    if let s = loadFixture(StateDoc.self, "state.json") {
        let hubs = s.config?.hubs ?? []
        check("state：config.hubs 维护者的在前、自己的在后，不认识的 kind 去掉了", hubs.map(\.kind) == [.maintainer, .own])
        let own = s.config?.ownHub
        check("state：自己的同步服务器没同步成，有原因、上次成功的时间、设备数、加入时间，能给加入链接",
              own?.state == .error && own?.state.failing == true && own?.error?.contains("连不上") == true && own?.checkedEpoch == 1794030000
              && own?.members == 3 && own?.joinedEpoch == 1793900000 && own?.canInvite == true && own?.host == "hub.example.com:18443")
        check("state：维护者的服务器没有设备数、加入时间、加入链接", hubs.first?.members == nil && hubs.first?.joinedEpoch == nil
              && hubs.first?.canInvite == false && hubs.first?.name == "studio-mac" && hubs.first?.accounts == 1)
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
    // 以后的 cca 给了不认识的 hub：当作 null（不经服务器同步），有「设备」一节也不当作维护者的
    if let d = fixture("state.json"), let text = String(data: d, encoding: .utf8) {
        let later = Data(text.replacingOccurrences(of: #""hub": "maintainer""#, with: #""hub": "later-kind""#).utf8)
        let s = try? DocumentDecoder.decode(StateDoc.self, from: later)
        let bob = s?.account(AccountRef(tool: .claude, key: "工作(bob@example.com)"))
        check("state：不认识的 hub 当作 null（有「设备」一节也一样），折合金额也是", bob != nil && bob?.can.devices == true && bob?.hub == nil
              && bob?.cost?.sync?.hub == nil)
    }
    if let doc = loadFixture(HubStatusDoc.self, "hub_status.json") {
        check("hub status：加入了、用得了、还没同步过", doc.joined && doc.reason == nil && doc.hub?.kind == .own && doc.hub?.state == .pending
              && doc.hub?.accounts == 2 && doc.hub?.members == 1 && doc.hub?.checkedEpoch == nil)
    }
    let notJoined = try? DocumentDecoder.decode(HubStatusDoc.self, from: Data(#"{"version":1,"kind":"hub_status","joined":false,"reason":"还没加入","hub":null}"#.utf8))
    check("hub status：没加入", notJoined?.joined == false && notJoined?.hub == nil && notJoined?.reason == "还没加入")
    if let doc = loadFixture(HubDevicesDoc.self, "hub_devices.json") {
        check("hub devices：这台在前；服务器重启以后没来过的没有 seen", doc.devices.map(\.name) == ["studio-mac", "carol-air"] && doc.devices[0].me
              && !doc.devices[1].me && doc.devices[1].seenEpoch == nil && doc.devices[0].id.hasPrefix("SHA256:") && doc.devices[0].joinedEpoch == 1793900000)
    }
    if let doc = loadFixture(DevicesDoc.self, "devices.json"), let a = doc.accounts.first {
        let h = doc.hub(for: a)
        check("旧版 cca 的 devices --json（没有 hubs、账号的 hub）：当作维护者的服务器，设置和名字用顶层的",
              doc.hubs.isEmpty && a.hub == nil && h.kind == .maintainer && h.me == doc.me && h.idleAfter == doc.idleAfter && h.offlineAfter == doc.offlineAfter)
    }
    let two = try? DocumentDecoder.decode(DevicesDoc.self, from: Data(#"""
        {"version":1,"kind":"devices","generated_epoch":1,"tool":"claude","me":"m-a","idle_after":600,"offline_after":150,"reporting_error":null,
         "hubs":[{"kind":"maintainer","me":"m-a","idle_after":600,"offline_after":150,"reporting_error":null},
                 {"kind":"own","me":"o-a","idle_after":300,"offline_after":90,"reporting_error":"连不上"}],
         "accounts":[{"key":"a(a@example.com)","hub":"maintainer","devices":[]},{"key":"b(b@example.com)","hub":"own","devices":[]}]}
        """#.utf8))
    if let two, two.accounts.count == 2 {
        let o = two.hub(for: two.accounts[1])
        check("devices --json 两台服务器：各账号按自己的 hub 用那一台的名字和设置", two.hub(for: two.accounts[0]).me == "m-a" && o.kind == .own && o.me == "o-a"
              && o.idleAfter == 300 && o.offlineAfter == 90 && o.reportingError == "连不上")
    } else {
        check("devices --json 两台服务器：解码了", false)
    }
    if let doc = loadFixture(CostSyncDoc.self, "cost_sync.json") {
        check("旧版 cca 的 cost sync --json（没有 hubs）：hubs 是空的", doc.sync?.hubs.isEmpty == true && doc.sync?.kind == nil)
    }
    let sync = try? DocumentDecoder.decode(CostSyncDoc.self, from: Data(#"""
        {"version":1,"kind":"cost_sync","generated_epoch":1,"scan":{"state":"ready","updated_epoch":1,"files":0,"read_bytes":0,"records":0,"first":false},
         "sync":{"state":"error","message":"连不上","checked_epoch":null,"hubs":[{"kind":"maintainer","state":"ok","message":null,"checked_epoch":5},
                                                                          {"kind":"own","state":"error","message":"连不上","checked_epoch":null}]}}
        """#.utf8))
    check("cost sync --json 两台服务器：顶层是最要紧的那一台，hubs 各是各的", sync?.sync?.state == .error && sync?.sync?.hubs.map(\.kind) == [.maintainer, .own]
          && sync?.sync?.hubs.first?.checkedEpoch == 5 && sync?.sync?.hubs.last?.message == "连不上")
    check("结果、失败、同意的代码", [ResultCode.hubJoined, .hubRemoved, .hubLeft].allSatisfy(\.isKnown)
          && [FailCode.hubNotJoined, .hubNoDevice, .hubUnavailable, .hubLink, .hubDenied, .hubSalt].allSatisfy(\.isKnown)
          && FailCode(rawValue: "hub_later").isHub && !FailCode(rawValue: "hub_later").isKnown && FailCode.hubPin.isKnown && !FailCode.devicesOff.isHub && [ConsentItem.hubRemove, .hubLeave].allSatisfy(\.isKnown))
    let homes = ["/Users/me/.cca"]
    let cases: [(String, CcaWatcher.Trigger?)] = [
        ("/Users/me/.cca/cost/own.remote.json", .state), ("/Users/me/.cca/cost/own.sync.json", .state), ("/Users/me/.cca/cost/own.lock", nil),
        ("/Users/me/.cca/hub/hub.json", .state), ("/Users/me/.cca/hub/members.json", .state), ("/Users/me/.cca/hub/id_ed25519", nil),
        ("/Users/me/.cca/hub/hub.json.tmp", nil), ("/Users/me/.cca/presence/status.json", .state), ("/Users/me/.cca/presence/own.status.json", .state),
        ("/Users/me/.cca/presence/keys.json", nil), ("/Users/me/.cca/presence/agent.json", nil), ("/Users/me/.cca/accounts/a/hub/hub.json", nil),
    ]
    let wrong = cases.filter { CcaWatcher.classify(path: $0.0, homes: homes) != $0.1 }.map(\.0)
    check("看着自己的同步服务器的加入信息、设备数、设备统计的状态、经它同步来的折合金额（别的不算）", wrong.isEmpty, wrong.joined(separator: " "))
}
