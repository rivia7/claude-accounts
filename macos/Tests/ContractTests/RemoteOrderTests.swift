import Foundation

// 「远程会话」页里会话的先后（拖动排的，`RemoteOrder`）：照记着的排、新的接在后面、拖一个、右键上移下移、记在 App 的设置里
// （`remote.order`），重新打开照旧；演示模式不写设置。

func remoteOrderTests() {
    section("远程会话的先后：RemoteOrder")
    let same = { (s: String) in s }
    var o = RemoteOrder()
    check("没排过：照原来的先后", o.sorted(["a", "b", "c"], id: same) == ["a", "b", "c"] && o.ids.isEmpty)
    o.record(["c", "a", "b"])
    check("排过的照排的先后", o.sorted(["a", "b", "c"], id: same) == ["c", "a", "b"])
    check("新的（没记着的）接在后面、照原来的先后；结束了的（不在了）不碍事", o.sorted(["d", "b", "e", "a"], id: same) == ["a", "b", "d", "e"])
    o.record(["x", "y"])
    check("另一台机器的一组排过：写在前面，前一组照旧", o.ids == ["x", "y", "c", "a", "b"] && o.sorted(["a", "b", "c"], id: same) == ["c", "a", "b"])
    o.record(["a", "c"])
    check("同一组再排：原来的位置去掉", o.ids == ["a", "c", "x", "y", "b"])
    let back = RemoteOrder(stored: o.stored)
    check("写进设置的是会话编号的 JSON 数组，读回来一样", back == o && o.stored == #"["a","c","x","y","b"]"#, o.stored)
    check("读不出来的：空的", RemoteOrder(stored: "不是 JSON").ids.isEmpty && RemoteOrder(stored: nil).ids.isEmpty
          && RemoteOrder(stored: #"{"a":1}"#).ids.isEmpty)
    var big = RemoteOrder()
    big.record((0..<300).map { "old\($0)" })
    big.record((0..<200).map { "new\($0)" })
    check("最多记 \(RemoteOrder.limit) 个：多了的从最后（早排过的）去掉", big.ids.count == RemoteOrder.limit && big.ids.first == "new0" && big.ids.last == "old199",
          "\(big.ids.count) \(big.ids.last ?? "")")
    let many = (try? JSONEncoder().encode((0..<500).map { "s\($0)" })).map { String(decoding: $0, as: UTF8.self) }
    check("读回来的也最多这么多个", RemoteOrder(stored: many).ids.count == RemoteOrder.limit)
}

@MainActor func remoteOrderModelTests() async {
    section("远程会话的先后：拖动、上移下移、记在 App 的设置里")
    guard let hello = fixture("hello.json").map({ String(decoding: $0, as: UTF8.self) }),
          let state = fixture("state.json").map({ String(decoding: $0, as: UTF8.self) }),
          let listData = fixture("remote_list.json"),
          var obj = (try? JSONSerialization.jsonObject(with: listData)) as? [String: Any],
          var rows = obj["sessions"] as? [[String: Any]], rows.count > 1 else {
        check("夹具", false)
        return
    }
    // office-linux 上再加一个会话：一组三个
    let s5c = String(repeating: "5c", count: 16), s7e = String(repeating: "7e", count: 16), s6d = String(repeating: "6d", count: 16)
    var extra = rows[1]
    extra["id"] = s6d
    extra["short"] = "6d6d6d6d"
    extra["cwd"] = "~/srv/web"
    rows.insert(extra, at: 3)
    obj["sessions"] = rows
    guard let listJSON = (try? JSONSerialization.data(withJSONObject: obj)).map({ String(decoding: $0, as: UTF8.self) }) else {
        check("夹具（加一个会话）", false)
        return
    }
    let t = ScriptedTransport { request, _ in
        let a = request.arguments
        if a == ["_gui", "hello"] { return response(0, stdout: hello) }
        if a == ["_gui", "state"] { return response(0, stdout: state) }
        if a.prefix(2) == ["remote", "list"] { return response(0, stdout: listJSON) }
        if a == ["remote", "template", "list", "--json"] { return response(0, stdout: #"{"version":1,"kind":"remote_templates","templates":[]}"#) }
        return response(0, stdout: SampleDocs.clientUpdates)
    }
    let prefs = MemoryPrefs()
    let scheduler = Scheduler()
    scheduler.remoteInterval = 3600
    let m = makeModel(t, prefs: prefs, scheduler: scheduler)
    await m.start()
    m.setMainVisible(true)
    m.selection = .remoteSessions
    let loaded = await waitUntil { m.remote.doc != nil && !m.remoteLoading }
    guard loaded, let doc = m.remote.doc else {
        check("列表", false)
        return
    }
    let office = doc.sessions.filter { $0.peer == "b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1" }
    let local = doc.sessions.filter(\.isLocal)
    func ids(_ g: [RemoteSession]) -> [String] { g.map(\.id) }
    func now() -> [RemoteSession] { m.orderedRemoteSessions(office) }
    check("没拖过：照 cca 给的先后", ids(office) == [s5c, s7e, s6d] && ids(now()) == [s5c, s7e, s6d] && prefs.string(forKey: "remote.order") == nil)
    check("往下拖到最后一个上：排到它的位置（它往上挪一格）", m.dropRemoteSession(s5c, onto: office[2], in: office) && ids(now()) == [s7e, s6d, s5c])
    check("记在 App 的设置里：这一组的会话编号（JSON 数组）", prefs.string(forKey: "remote.order") == #"["\#(s7e)","\#(s6d)","\#(s5c)"]"#,
          prefs.string(forKey: "remote.order") ?? "nil")
    var g = now()
    check("往上拖到第一个上：排到它的位置", m.dropRemoteSession(s5c, onto: g[0], in: g) && ids(now()) == [s5c, s7e, s6d])
    g = now()
    check("拖到自己身上、拖别的机器上的会话过来：不动", !m.dropRemoteSession(s5c, onto: g[0], in: g) && !m.dropRemoteSession(local[0].id, onto: g[1], in: g)
          && !m.dropRemoteSession("zz", onto: g[1], in: g) && ids(now()) == [s5c, s7e, s6d])
    m.stepRemoteSession(g[2], by: -1, in: g)
    check("右键「上移」：和上面的一个换位置", ids(now()) == [s5c, s6d, s7e])
    g = now()
    m.stepRemoteSession(g[0], by: -1, in: g)
    m.stepRemoteSession(g[2], by: 1, in: g)
    check("第一个不能再上移、最后一个不能再下移", ids(now()) == [s5c, s6d, s7e])
    m.stepRemoteSession(g[0], by: 1, in: g)
    check("「下移」", ids(now()) == [s6d, s5c, s7e])
    check("别的机器上的、这台 Mac 上的会话：照 cca 给的先后（不受影响）", ids(m.orderedRemoteSessions(local)) == ids(local))
    var fresh = office[0]
    fresh.id = String(repeating: "0f", count: 16)
    check("新起的会话（没排过，cca 排在前面也一样）：接在后面", ids(m.orderedRemoteSessions([fresh] + office)) == [s6d, s5c, s7e, fresh.id])
    m.shutdown()

    let again = makeModel(t, prefs: prefs, scheduler: scheduler)
    check("重新打开 App：照记着的先后", ids(again.orderedRemoteSessions(office)) == [s6d, s5c, s7e])
    again.shutdown()

    let demoPrefs = MemoryPrefs()
    let demo = makeModel(fixtureTransport(), prefs: demoPrefs)
    check("演示模式：拖了照样排，不写设置", demo.isFixtureMode && demo.dropRemoteSession(s5c, onto: office[2], in: office)
          && ids(demo.orderedRemoteSessions(office)) == [s7e, s6d, s5c] && demoPrefs.string(forKey: "remote.order") == nil)
    demo.shutdown()
}
