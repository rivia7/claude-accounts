// 测试用的手机那一头（macOS 上的命令行程序，和 App 编进同一份 CcaRemoteKit）：tests/ios_kit.rs 拿它和真的服务器、真的 cca 对着测。
// 用法：ccaguest --store 目录 --name 名字 <命令>
//   pair <链接>             配对；stdout：peer 配对编号 名字，没成是 fail 原因（退出码 1）
//   list                    一行一个 JSON：配对过的机器（没有 sid；served：服务器转不转账号的状态，status：那台机器放上来的账号的状态，
//                           没有是 null；launcher：那台机器的启动器 {sid, online, machine, more, t: [模板 {i, h, n, k, a, aid, d, x, l}]}，
//                           没有是 null）、它上面的会话（有 sid）
//   attach 配对编号 会话编号 [--view] [--size 列x行] [--fresh]   接入（窗口大小默认 80x24）：stdin 是敲的键（读到头就断开），stdout 是终端的
//                           字节；stderr：state open|retrying、size 会话的列 行（宿主说的）、exit 退出码、note 通知的 JSON、closed 原因。
//                           detached、ended 退出码 0，别的 1。--fresh：刚远程启动的会话（服务器说没有它时 45 秒里接着问）
//   launch 配对编号 模板编号 [--size 列x行] [--line 开场白]   远程启动（大小默认 80x24）：先列一遍找那台机器的启动器；stdout：
//                           launched 新会话的编号，没起是 refused 原因码 原因、没问到是 fail 原因（退出码 1）
//   unpair 配对编号          取消配对；stdout：unpaired（服务器没问到也删本地的：told-no）
//   push-set 令牌 环境 [--only 会话编号,…] [--usage]   在配对的机器所在的每台服务器上登记推送（令牌空的是拿掉；--only：只推这几个
//                           会话，空的是一个都不推；--usage：也推额度的通知）；stdout 一台一行：push 服务器 ok|fail 原因（带了 --only 时
//                           ok 后面是 only：服务器认了，all：旧的服务器不认、照旧都推；带了 --usage 时再跟 usage：服务器认了，no-usage：不认）
//   push-open 文件           文件里是推来的内容（推送服务收到的那份 JSON）：像通知扩展那样拿每个配对的通知密钥试着解开；
//                           stdout：解开的通知和弹出来的字（一行 JSON），解不开是 fail（退出码 1）
// 密钥存在目录里的文件里（只给测试用；App 里存钥匙串）
import Foundation

final class DirStore: RemoteStore {
    let dir: URL

    init(_ dir: URL) {
        self.dir = dir
    }

    private func file(_ key: String) -> URL {
        dir.appendingPathComponent("secret-" + sha256(Bytes(utf8: key)).hex.prefix(24))
    }

    func secret(_ key: String) -> Bytes? {
        (try? Data(contentsOf: file(key))).map { Bytes($0) }
    }

    func setSecret(_ key: String, _ value: Bytes?) throws {
        if let value {
            try Data(value).write(to: file(key), options: .atomic)
        } else {
            try? FileManager.default.removeItem(at: file(key))
        }
    }

    func peersData() -> Data? {
        try? Data(contentsOf: dir.appendingPathComponent("peers.json"))
    }

    func setPeersData(_ data: Data) throws {
        try data.write(to: dir.appendingPathComponent("peers.json"), options: .atomic)
    }
}

func say(_ s: String) {
    FileHandle.standardError.write(Data((s + "\n").utf8))
}

/// 一个账号的折合金额：读成模型以后的样子（按周、按天的每一项是 [开始, 结束, 美分, 是不是这一项, 开始时还没有记录]）
func costJSON(_ c: AccountCost?) -> Any {
    guard let c else { return NSNull() }
    func periods(_ l: [CostPeriod]) -> [[Any]] { l.map { [$0.startEpoch, $0.endEpoch, $0.cents, $0.current, $0.partial] } }
    let models: Any = c.models.map { $0.map { m -> [Any] in [m.name ?? NSNull(), m.weeks, m.days, m.pricedAs ?? NSNull()] } } ?? NSNull()
    return ["since": c.sinceEpoch ?? NSNull(), "weeks": periods(c.weeks), "days": periods(c.days), "sum": [c.weeksTotal, c.daysTotal],
            "average": [c.averageWeek ?? NSNull(), c.averageDay ?? NSNull()] as [Any], "average_n": [c.averageWeeks, c.averageDays],
            "cache_hit": [c.cacheHitWeeks ?? NSNull(), c.cacheHitDays ?? NSNull()] as [Any], "sync": c.sync, "missing": c.missing,
            "models": models, "devices": c.devices.map { d -> [Any] in [d.name, d.me, d.weeks, d.days, d.stale, d.syncedEpoch ?? NSNull()] }]
        as [String: Any]
}

/// list 里一台机器放上来的账号的状态：解开、读成模型以后的样子（测试照它核对）
func statusJSON(_ s: HostStatus, now: Double) -> [String: Any] {
    let accounts: [[String: Any]] = s.accounts.map { a in
        func win(_ w: UsageWindow?) -> Any {
            guard let w else { return NSNull() }
            return ["utilization": w.utilization ?? NSNull(), "resets_epoch": w.resetsEpoch ?? NSNull(), "reset": w.reset] as [String: Any]
        }
        let used = a.exhausted(now: now).map { [$0.slot.rawValue, $0.resetsEpoch ?? NSNull()] as [Any] }
        return ["id": a.id, "aid": a.aid, "tool": a.tool, "name": a.name, "kind": a.kind, "login": a.login, "plan": a.plan, "tier": a.tier,
                "busy": a.busy, "color": a.color ?? NSNull(), "badge": a.badge, "source": a.usage?.source ?? NSNull(),
                "five_hour": win(a.usage?.fiveHour), "seven_day": win(a.usage?.sevenDay), "weeks": a.weeks?.weeks.count ?? NSNull(),
                "exhausted": used, "app": a.appOpen, "cli": a.cliSessions, "since": a.cliSince ?? NSNull(), "cost": costJSON(a.cost)]
    }
    let cost: Any = s.cost.map { c -> [String: Any] in
        ["prices_date": c.pricesDate, "updated": c.updatedEpoch ?? NSNull(), "scanning": c.scanning, "unattributed": c.unattributed, "brief": c.brief]
    } ?? NSNull()
    let team: Any = s.team.map { t -> [String: Any] in
        ["name": t.name, "role": t.role, "coadmin": t.coadmin, "state": t.state, "error": t.error, "seq": t.seq, "accounts": t.accounts,
         "device": t.device, "host": t.host, "joined": t.joinedEpoch ?? NSNull(), "update": t.update ?? NSNull()]
    } ?? NSNull()
    return ["machine": s.machine, "off": s.off, "more": s.more, "at": s.at, "age": s.age, "accounts": accounts, "weeks_left_out": s.weeksLeftOut,
            "cost": cost, "cost_left_out": s.costLeftOut, "team": team]
}

var args = Array(CommandLine.arguments.dropFirst())
var store = "", name = "iPhone"
while args.count >= 2, args[0].hasPrefix("--") {
    if args[0] == "--store" { store = args[1] } else if args[0] == "--name" { name = args[1] }
    args.removeFirst(2)
}
guard !store.isEmpty, let cmd = args.first else {
    say("用法：ccaguest --store 目录 [--name 名字] pair|list|attach|launch|unpair|push-set|push-open …")
    exit(64)
}
let client = RemoteClient(store: DirStore(URL(fileURLWithPath: store)), deviceName: name)

/// list 里一台机器的启动器：解开、清理以后的样子（测试照它核对）
func launcherJSON(_ l: HostLauncher) -> [String: Any] {
    let t: [[String: Any]] = l.info.templates.map { x in
        ["i": x.id, "h": x.digest, "n": x.name, "k": x.tool, "a": x.account, "aid": x.aid, "d": x.dir, "x": x.args, "l": x.line]
    }
    return ["sid": l.sid, "online": l.online, "machine": l.info.machine, "more": l.info.more, "t": t]
}

/// --size 列x行（没有、不对是 80x24）
func sizeArg(_ args: [String]) -> (Int, Int) {
    guard let i = args.firstIndex(of: "--size"), i + 1 < args.count else { return (80, 24) }
    let p = args[i + 1].split(separator: "x").compactMap { Int($0) }
    return p.count == 2 ? (p[0], p[1]) : (80, 24)
}

func finish(_ code: Int32) -> Never {
    fflush(stdout)
    exit(code)
}

switch cmd {
case "pair":
    guard args.count == 2, let inv = PairInvite(args[1]) else {
        print("fail link")
        finish(1)
    }
    Task {
        do {
            let peer = try await client.pair(inv)
            print("peer \(peer.pid) \(peer.name)")
            finish(0)
        } catch {
            print("fail \((error as? RemoteFailure)?.code ?? "error")")
            finish(1)
        }
    }
case "list":
    Task {
        let now = Date().timeIntervalSince1970
        for row in await client.list() {
            let base: [String: Any] = ["pid": row.peer.pid, "peer": row.peer.name, "gone": row.gone, "failure": row.failure?.code ?? NSNull()]
            let head = base.merging(["served": row.statusServed, "status": row.status.map { statusJSON($0, now: now) } ?? NSNull(),
                                     "launcher": row.launcher.map(launcherJSON) ?? NSNull()]) { $1 }
            print(String(decoding: try! JSONSerialization.data(withJSONObject: head, options: [.sortedKeys]), as: UTF8.self))
            for s in row.sessions {
                let j = base.merging(["sid": s.sid, "machine": s.info.machine, "tool": s.info.tool, "name": s.info.name, "cwd": s.info.cwd,
                                      "online": s.online, "view": s.info.viewOnly, "viewers": s.viewers, "aid": s.info.aid]) { $1 }
                print(String(decoding: try! JSONSerialization.data(withJSONObject: j, options: [.sortedKeys]), as: UTF8.self))
            }
        }
        finish(0)
    }
case "unpair":
    guard args.count == 2, let peer = client.peers().first(where: { $0.pid == args[1] }) else {
        print("fail no_peer")
        finish(1)
    }
    Task {
        print(await client.unpair(peer) ? "unpaired" : "told-no")
        finish(0)
    }
case "push-set":
    let usage = args.last == "--usage"
    if usage { args.removeLast() }
    guard args.count == 3 || (args.count == 5 && args[3] == "--only") else {
        print("fail usage")
        finish(1)
    }
    let sids: [String]? = args.count == 5 ? args[4].split(separator: ",").map(String.init) : nil
    Task {
        var all = true
        for s in client.servers() {
            switch await client.pushSet(on: s, token: args[1], env: args[2], sids: sids, usage: usage ? true : nil) {
            case .ok(let filtered, let usageOK):
                print("push \(s.url) ok" + (sids == nil ? "" : filtered ? " only" : " all") + (usage ? usageOK ? " usage" : " no-usage" : ""))
            case .failed(let f):
                print("push \(s.url) fail \(f.code)")
                all = false
            }
        }
        finish(all ? 0 : 1)
    }
case "push-open":
    guard args.count == 2, let raw = FileManager.default.contents(atPath: args[1]),
          let info = (try? JSONSerialization.jsonObject(with: raw)) as? [String: Any]
    else {
        print("fail usage")
        finish(1)
    }
    guard let n = openPushed(info, keys: client.noteKeys()), let card = NoteCard(n) else {
        print("fail")
        finish(1)
    }
    var j: [String: Any] = ["pid": n.pid, "machine": n.machine, "id": n.id, "sid": n.sid, "at": n.at, "title": n.title,
                            "subtitle": n.subtitle, "body": n.body, "card": [card.title, card.subtitle, card.body]]
    if let u = n.usage {
        j["usage"] = ["event": u.event.rawValue, "account": u.account, "aid": u.aid, "tool": u.tool, "name": u.name, "window": u.slot.rawValue,
                      "resets_epoch": u.resetsEpoch ?? NSNull()] as [String: Any]
    }
    print(String(decoding: try! JSONSerialization.data(withJSONObject: j, options: [.sortedKeys]), as: UTF8.self))
    finish(0)
case "attach":
    guard args.count >= 3, let peer = client.peers().first(where: { $0.pid == args[1] }) else {
        print("fail no_peer")
        finish(1)
    }
    let (cols, rows) = sizeArg(args)
    let att = Attachment(client: client, peer: peer, sid: args[2], cols: cols, rows: rows, readOnly: args.contains("--view"),
                         fresh: args.contains("--fresh"))
    att.onOutput = { b in
        FileHandle.standardOutput.write(Data(b))
    }
    att.onSize = { c, r in say("size \(c) \(r)") }
    att.onExit = { say("exit \($0)") }
    att.onNote = { n in
        var j = ["title": n.title, "subtitle": n.subtitle, "body": n.body]
        if !n.id.isEmpty { j["id"] = n.id }
        say("note " + String(decoding: try! JSONSerialization.data(withJSONObject: j, options: [.sortedKeys]), as: UTF8.self))
    }
    var reading = false
    att.onState = { st in
        switch st {
        case .open:
            say("state open")
            guard !reading else { return }
            reading = true
            // 敲的键：stdin 读到什么送什么，读到头就断开
            Thread.detachNewThread {
                while true {
                    let d = FileHandle.standardInput.availableData
                    if d.isEmpty { break }
                    DispatchQueue.main.async { _ = att.input(Bytes(d)) }
                }
                DispatchQueue.main.async { att.detach() }
            }
        case .connecting:
            say("state connecting")
        case .retrying:
            say("state retrying")
        case .closed(let why):
            say("closed \(why)")
            finish(why == "detached" || why == "ended" ? 0 : 1)
        }
    }
    att.start()
case "launch":
    guard args.count >= 3, let peer = client.peers().first(where: { $0.pid == args[1] }) else {
        print("fail no_peer")
        finish(1)
    }
    let (cols, rows) = sizeArg(args)
    let line = args.firstIndex(of: "--line").flatMap { $0 + 1 < args.count ? args[$0 + 1] : nil }
    Task {
        guard let l = await client.list().first(where: { $0.peer.pid == peer.pid })?.launcher else {
            print("fail no_launcher")
            finish(1)
        }
        guard let t = l.info.templates.first(where: { $0.id == args[2] }) else {
            print("fail no_template")
            finish(1)
        }
        switch await client.launch(peer, launcher: l.sid, template: t, cols: cols, rows: rows, line: line) {
        case .launched(let sid):
            print("launched \(sid)")
            finish(0)
        case .refused(let code, let text):
            print("refused \(code) \(text)")
            finish(1)
        case .failed(let f):
            print("fail \(f.code)")
            finish(1)
        }
    }
default:
    say("不认识的命令 \(cmd)")
    exit(64)
}
dispatchMain()
