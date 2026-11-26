// 测试用的手机那一头（macOS 上的命令行程序，和 App 编进同一份 CcaRemoteKit）：tests/ios_kit.rs 拿它和真的服务器、真的 cca 对着测。
// 用法：ccaguest --store 目录 --name 名字 <命令>
//   pair <链接>             配对；stdout：peer 配对编号 名字，没成是 fail 原因（退出码 1）
//   list                    一行一个 JSON：配对过的机器（没有 sid）、它上面的会话（有 sid）
//   attach 配对编号 会话编号 [--view] [--size 列x行]   接入（窗口大小默认 80x24）：stdin 是敲的键（读到头就断开），stdout 是终端的字节；
//                           stderr：state open|retrying、size 会话的列 行（宿主说的）、exit 退出码、note 通知的 JSON、closed 原因。
//                           detached、ended 退出码 0，别的 1
//   unpair 配对编号          取消配对；stdout：unpaired（服务器没问到也删本地的：told-no）
//   push-set 令牌 环境 [--only 会话编号,…]   在配对的机器所在的每台服务器上登记推送（令牌空的是拿掉；--only：只推这几个会话，
//                           空的是一个都不推）；stdout 一台一行：push 服务器 ok|fail 原因（带了 --only 时 ok 后面是 only：服务器认了，
//                           all：旧的服务器不认、照旧都推）
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

var args = Array(CommandLine.arguments.dropFirst())
var store = "", name = "iPhone"
while args.count >= 2, args[0].hasPrefix("--") {
    if args[0] == "--store" { store = args[1] } else if args[0] == "--name" { name = args[1] }
    args.removeFirst(2)
}
guard !store.isEmpty, let cmd = args.first else {
    say("用法：ccaguest --store 目录 [--name 名字] pair|list|attach|unpair|push-set|push-open …")
    exit(64)
}
let client = RemoteClient(store: DirStore(URL(fileURLWithPath: store)), deviceName: name)

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
        for row in await client.list() {
            let base: [String: Any] = ["pid": row.peer.pid, "peer": row.peer.name, "gone": row.gone, "failure": row.failure?.code ?? NSNull()]
            print(String(decoding: try! JSONSerialization.data(withJSONObject: base, options: [.sortedKeys]), as: UTF8.self))
            for s in row.sessions {
                let j = base.merging(["sid": s.sid, "machine": s.info.machine, "tool": s.info.tool, "name": s.info.name, "cwd": s.info.cwd,
                                      "online": s.online, "view": s.info.viewOnly, "viewers": s.viewers]) { $1 }
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
    guard args.count == 3 || (args.count == 5 && args[3] == "--only") else {
        print("fail usage")
        finish(1)
    }
    let sids: [String]? = args.count == 5 ? args[4].split(separator: ",").map(String.init) : nil
    Task {
        var all = true
        for s in client.servers() {
            switch await client.pushSet(on: s, token: args[1], env: args[2], sids: sids) {
            case .ok(let filtered):
                print("push \(s.url) ok" + (sids == nil ? "" : filtered ? " only" : " all"))
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
    let j: [String: Any] = ["pid": n.pid, "machine": n.machine, "id": n.id, "sid": n.sid, "at": n.at, "title": n.title,
                            "subtitle": n.subtitle, "body": n.body, "card": [card.title, card.subtitle, card.body]]
    print(String(decoding: try! JSONSerialization.data(withJSONObject: j, options: [.sortedKeys]), as: UTF8.self))
    finish(0)
case "attach":
    guard args.count >= 3, let peer = client.peers().first(where: { $0.pid == args[1] }) else {
        print("fail no_peer")
        finish(1)
    }
    var (cols, rows) = (80, 24)
    if let i = args.firstIndex(of: "--size"), i + 1 < args.count {
        let p = args[i + 1].split(separator: "x").compactMap { Int($0) }
        if p.count == 2 { (cols, rows) = (p[0], p[1]) }
    }
    let att = Attachment(client: client, peer: peer, sid: args[2], cols: cols, rows: rows, readOnly: args.contains("--view"))
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
default:
    say("不认识的命令 \(cmd)")
    exit(64)
}
dispatchMain()
