// 客户端：身份、配对表、配对、列会话、接入、远程启动。和 server/web/core.js 的 Client 是同一套流程（那一份以后去掉；远程启动它没有）。
import Foundation

/// 配对过的一台机器（这台手机能接入它上面的会话）
public struct Peer: Codable, Hashable, Identifiable, Sendable {
    /// 配对编号（服务器只知道它）
    public let pid: String
    /// 那台机器的指纹（服务器验过签名的、和它在密文里说的一样）
    public let fp: String
    /// 给人看的名字
    public let name: String
    /// 什么时候配的（秒）
    public let at: Double
    /// 配对记在哪台服务器上：经它接入
    public let server: ServerRef
    /// 那台服务器是哪个 Team 的（名字，给人看的；维护者的服务器没有）
    public let team: String?

    public var id: String { pid }

    public init(pid: String, fp: String, name: String, at: Double, server: ServerRef, team: String?) {
        self.pid = pid
        self.fp = fp
        self.name = name
        self.at = at
        self.server = server
        self.team = team
    }
}

/// 存东西的地方。密钥（这台设备在每台服务器上的身份、每个配对的密钥 P）App 里存钥匙串，配对表（不是秘密）存文件；
/// 测试、命令行里是一个目录
public protocol RemoteStore: AnyObject {
    func secret(_ key: String) -> Bytes?
    func setSecret(_ key: String, _ value: Bytes?) throws
    func peersData() -> Data?
    func setPeersData(_ data: Data) throws
}

/// 配对的进度：算密钥（PBKDF2，要一会儿）、问服务器、等对方确认
public enum PairStep: Sendable, Equatable {
    case deriving, asking, confirming
}

/// 一台机器和它上面这台手机能接入的会话、它放上来的账号的状态
public struct PeerListing: Sendable {
    public let peer: Peer
    /// 对方已经取消了配对
    public let gone: Bool
    public let sessions: [RemoteSession]
    /// 没问到这台机器所在的服务器（原因）
    public let failure: RemoteFailure?
    /// 那台机器放上来的账号的状态（Status.swift）：还没放过（旧版的 cca、刚配好还没查过用量）、服务器重启过（它只在内存里存着）、
    /// 解不开时是 nil
    public let status: HostStatus?
    /// 那台服务器转不转账号的状态（3.7.5 起的才转：旧的服务器上 status 总是 nil，要管理员更新服务器）
    public let statusServed: Bool
    /// 那台机器的启动器（Launch.swift）：这台手机能照它的模板远程启动会话。那台机器没让这台手机远程启动、旧版的 cca 是 nil
    public let launcher: HostLauncher?

    public init(peer: Peer, gone: Bool, sessions: [RemoteSession], failure: RemoteFailure?, status: HostStatus? = nil,
                statusServed: Bool = false, launcher: HostLauncher? = nil) {
        self.peer = peer
        self.gone = gone
        self.sessions = sessions
        self.failure = failure
        self.status = status
        self.statusServed = statusServed
        self.launcher = launcher
    }
}

public struct RemoteSession: Identifiable, Hashable, Sendable {
    public let sid: String
    public let info: SessionInfo
    /// 那台机器这会儿连着服务器
    public let online: Bool
    /// 现在有几处接着
    public let viewers: Int

    public var id: String { sid }

    public init(sid: String, info: SessionInfo, online: Bool, viewers: Int) {
        self.sid = sid
        self.info = info
        self.online = online
        self.viewers = viewers
    }
}

extension SessionInfo: Hashable {
    public func hash(into h: inout Hasher) {
        h.combine(machine)
        h.combine(name)
        h.combine(cwd)
        h.combine(since)
    }
}

public final class RemoteClient: @unchecked Sendable {
    public static let peerName = 48
    private let store: RemoteStore
    private let lock = NSLock()
    private var requesters = [String: Requester]()
    /// 解开过的账号的状态（配对编号 → 那一份的 n 和解开的）：查的时候告诉服务器手里有哪一份（have），一样的它就不再发
    private var statuses = [String: (n: String, status: HostStatus)]()
    /// 配对时告诉对方这台设备叫什么
    public var deviceName: String

    public init(store: RemoteStore, deviceName: String) {
        self.store = store
        self.deviceName = deviceName
    }

    // MARK: 身份、配对表

    private func idKey(_ s: ServerRef) -> String { "id " + s.url }
    private func pKey(_ pid: String) -> String { "p " + pid }

    /// 这台设备在这台服务器上的身份：第一次用时生成。每台服务器一把（几台服务器之间认不出是同一台手机）
    func identity(for server: ServerRef) throws -> Identity {
        if let raw = store.secret(idKey(server)), let id = Identity(raw: raw) {
            return id
        }
        let raw = Identity.generate()
        try store.setSecret(idKey(server), raw)
        guard let id = Identity(raw: raw) else { throw RemoteFailure("error") }
        return id
    }

    func requester(_ server: ServerRef) throws -> Requester {
        lock.lock()
        defer { lock.unlock() }
        if let r = requesters[server.url], r.server == server {
            return r
        }
        let r = Requester(server: server, identity: try identity(for: server))
        requesters[server.url] = r
        return r
    }

    public func peers() -> [Peer] {
        guard let d = store.peersData(), let all = try? JSONDecoder().decode([Peer].self, from: d) else { return [] }
        return all
    }

    private func setPeers(_ all: [Peer]) throws {
        try store.setPeersData(try JSONEncoder().encode(all))
    }

    /// 这台设备在这台服务器上的指纹（设置页里显示前几位，电脑那头 `cca remote peers` 看得到同一个）
    public func fingerprint(on server: ServerRef) -> String? {
        try? identity(for: server).fp
    }

    // MARK: 推送

    /// 这台手机上每个配对的通知密钥（App 把它们存进和通知扩展共用的钥匙串分组：NoteKeychain）
    public func noteKeys() -> [NoteKey] {
        peers().compactMap { p in store.secret(pKey(p.pid)).map { NoteKey(pid: p.pid, p: $0, name: p.name) } }
    }

    /// 配对的机器所在的服务器，每台一次（照配对表的先后）
    public func servers() -> [ServerRef] {
        var seen = Set<String>()
        return peers().map(\.server).filter { seen.insert($0.url).inserted }
    }

    /// 在这台服务器上登记推送（push_set，只有手机能发）：推送服务给这台手机上这个 App 的令牌（小写的十六进制）和推送服务的环境
    /// （production / sandbox）；令牌空的是拿掉。sids：只推这几个会话（会话编号，最多 8 个；空的是一个都不推：「已关闭的会话也推送」
    /// 关着时是 App 里开着的那个），nil 是都推。usage：也推额度的通知（额度用完了、恢复了），nil 是不说（服务器当作不推）。
    /// 没成的原因：no_push（这台服务器没接推送网关）、denied（服务器不认这台手机了）……
    public func pushSet(on server: ServerRef, token: String, env: String, sids: [String]? = nil, usage: Bool? = nil) async -> PushSetResult {
        var body: [String: Any] = ["token": token, "env": env]
        if let sids { body["sids"] = sids }
        if let usage { body["usage"] = usage }
        do {
            let j = try await requester(server).post("push_set", body)
            return .ok(filtered: sids != nil && j["sids"] as? Bool == true, usage: usage != nil && j["usage"] as? Bool == true)
        } catch {
            return .failed(error as? RemoteFailure ?? RemoteFailure("error"))
        }
    }

    // MARK: 配对

    /// 扫到的二维码（或者粘贴的链接）里的配对码：和那台机器配对。回配对表里新的那一项
    public func pair(_ invite: PairInvite, progress: @Sendable (PairStep) -> Void = { _ in }) async throws -> Peer {
        let req = try requester(invite.server)
        let me = req.identity
        progress(.deriving)
        let k = await Task.detached(priority: .userInitiated) { pairKeys(invite.code) }.value
        let nb = random(32)
        progress(.asking)
        do {
            // 带上公钥：这台设备不在服务器的设备清单里，凭这次配对（知道配对码才知道 rid）让服务器认它
            _ = try await req.post("pair_put", ["rid": k.rid, "pub": me.pub,
                                                "m": pairMsg(k.keys, rid: k.rid, role: "b", fp: me.fp, nonce: nb, name: deviceName)])
        } catch let e as RemoteFailure where [400, 403, 404, 409].contains(e.status) {
            // 没有这个配对码（过期了、用过了）、它不是开给手机的：服务器对不认识的设备回的都一样
            throw RemoteFailure("no_pairing", status: e.status)
        }
        var committed: String?
        do {
            var theirs: (m: Any, fp: String, listed: String)?
            for _ in 0..<4 where theirs == nil {
                let j = try await req.post("pair_get", ["rid": k.rid, "wait": 10])
                if let m = j["m"], !(m is NSNull), let fp = j["fp"] as? String {
                    theirs = (m, fp, j["name"] as? String ?? "")
                }
            }
            guard let theirs else { throw RemoteFailure("no_answer") }
            guard let opened = pairOpen(k.keys, rid: k.rid, role: "a", theirs.m), opened.fp == theirs.fp, opened.fp != me.fp else {
                throw RemoteFailure("bad_peer")
            }
            let (p, pid) = pairSecret(k.kc, na: opened.n, nb: nb, fpa: opened.fp, fpb: me.fp)
            progress(.confirming)
            var done = try await req.post("pair_commit", ["rid": k.rid, "pid": pid])["done"] as? Bool == true
            for _ in 0..<6 where !done {
                done = try await req.post("pair_get", ["rid": k.rid, "wait": 10, "until": "done"])["done"] as? Bool == true
            }
            guard done else { throw RemoteFailure("no_answer") }
            committed = pid
            // 名字先用服务器的设备清单里那台机器叫什么（访客问到的总是空的），再是它自己说的、二维码里写的
            let named = [theirs.listed, opened.name, invite.machine].map { clean($0, Self.peerName) }.first { !$0.isEmpty }
            let name = named ?? "电脑 \(pid.prefix(6))"
            let peer = Peer(pid: pid, fp: opened.fp, name: name, at: Date().timeIntervalSince1970, server: invite.server, team: invite.team)
            try store.setSecret(pKey(pid), p)
            // 同一台服务器上的同一台机器以前配过的换掉（服务器上也是换掉）；别的服务器上的照旧
            var all = peers()
            let old = all.filter { ($0.fp == peer.fp && $0.server.url == peer.server.url) || $0.pid == pid }
            all.removeAll { x in old.contains { $0.pid == x.pid } }
            all.append(peer)
            try setPeers(all)
            for o in old where o.pid != pid {
                try? store.setSecret(pKey(o.pid), nil)
            }
            return peer
        } catch {
            // 服务器已经记下了、这边却没存成：把那条配对删掉，不留一条这台设备用不了的；没配成：告诉服务器这次作废
            if let committed {
                try? store.setSecret(pKey(committed), nil)
                _ = try? await req.post("pair_drop", ["pid": committed])
            } else {
                _ = try? await req.post("pair_close", ["rid": k.rid])
            }
            throw error
        }
    }

    /// 取消配对：服务器上的记录删掉（删不成也不拦着，回 false），这台设备上的密钥、配对表里的那一项删掉
    @discardableResult
    public func unpair(_ peer: Peer) async -> Bool {
        var told = true
        do {
            _ = try await requester(peer.server).post("pair_drop", ["pid": peer.pid])
        } catch {
            told = false
        }
        try? setPeers(peers().filter { $0.pid != peer.pid })
        try? store.setSecret(pKey(peer.pid), nil)
        return told
    }

    // MARK: 会话

    /// 配对过的机器和它们上面可以接入的会话（照配对表的先后）。每台服务器问一次
    public func list() async -> [PeerListing] {
        let all = peers()
        var byServer = [ServerRef: [Peer]]()
        for p in all {
            byServer[p.server, default: []].append(p)
        }
        var out = [String: PeerListing]()
        await withTaskGroup(of: [PeerListing].self) { group in
            for (server, ps) in byServer {
                group.addTask { await self.list(on: server, ps) }
            }
            for await rows in group {
                for r in rows {
                    out[r.peer.pid] = r
                }
            }
        }
        return all.compactMap { out[$0.pid] }
    }

    private func list(on server: ServerRef, _ ps: [Peer]) async -> [PeerListing] {
        let j: [String: Any]
        // 手里有的账号的状态是哪一份（可以是空的：带了 have 的，3.7.5 起的服务器才回 status）
        let have = lock.withLock { Dictionary(ps.compactMap { p in statuses[p.pid].map { (p.pid, $0.n) } }, uniquingKeysWith: { a, _ in a }) }
        do {
            j = try await requester(server).post("relay_list", ["pids": ps.map(\.pid), "have": have])
        } catch {
            let f = error as? RemoteFailure ?? RemoteFailure("error")
            // 没问到：账号的状态照手里那一份（界面上写着是多久以前的）
            return ps.map { p in PeerListing(peer: p, gone: false, sessions: [], failure: f, status: lock.withLock { statuses[p.pid]?.status }) }
        }
        let known = j["peers"] as? [String: Any] ?? [:]
        let rows = j["sessions"] as? [String: Any] ?? [:]
        let served = j["status"] is [String: Any]
        let status = j["status"] as? [String: Any] ?? [:]
        let now = Date().timeIntervalSince1970
        return ps.map { peer in
            guard let p = store.secret(pKey(peer.pid)) else {
                return PeerListing(peer: peer, gone: known[peer.pid] == nil, sessions: [], failure: RemoteFailure("no_key"), statusServed: served)
            }
            let keys = dirKeys(p), lkeys = launchKeys(p)
            var sessions = [RemoteSession]()
            var launcher: HostLauncher?
            for r in rows[peer.pid] as? [Any] ?? [] {
                guard let r = r as? [String: Any], let sid = r["sid"] as? String, sid.count == 32, Bytes.unhex(sid) != nil else { continue }
                let online = r["online"] as? Bool == true
                if let info = dirOpen(keys, pid: peer.pid, sid: sid, r["blob"]) {
                    sessions.append(RemoteSession(sid: sid, info: info, online: online, viewers: jsonInt(r["viewers"]) ?? 0))
                } else if launcher == nil, let info = launchOpen(lkeys, pid: peer.pid, sid: sid, r["blob"]) {
                    // 那台机器的启动器：在服务器上它也是一个“会话”，说明是另一把密钥封的（有几个时认头一个）
                    launcher = HostLauncher(sid: sid, info: info, online: online)
                }
                // 都解不开的：不是这个配对的密钥封的（服务器、别人塞进来的），不显示
            }
            sessions.sort { $0.info.since > $1.info.since }
            let st = known[peer.pid] == nil ? nil : statusOf(peer.pid, p: p, row: status[peer.pid], now: now)
            return PeerListing(peer: peer, gone: known[peer.pid] == nil, sessions: sessions, failure: nil, status: st, statusServed: served,
                               launcher: launcher)
        }
    }

    /// relay_list 回的一个配对的状态（`{"blob", "age"}`，和手里那一份一样的是 `{"same": true, "age"}`）：解开、记下；没有的（还没放过、
    /// 服务器重启过、对方取消了配对）、解不开的把手里那一份也忘掉
    private func statusOf(_ pid: String, p: Bytes, row: Any?, now: Double) -> HostStatus? {
        let r = row as? [String: Any]
        let age = r.flatMap { finiteNumber($0["age"]) } ?? 0
        if let r, jsonBool(r["same"]) == true {
            return lock.withLock {
                guard var s = statuses[pid] else { return nil }
                s.status.age = max(age, 0)
                s.status.received = now
                statuses[pid] = s
                return s.status
            }
        }
        guard let blob = r?["blob"] as? [String: Any], let n = blob["n"] as? String,
              let s = statusOpen(statusKeys(p), pid: pid, blob, age: age, received: now)
        else {
            lock.withLock { statuses[pid] = nil }
            return nil
        }
        lock.withLock { statuses[pid] = (n, s) }
        return s
    }

    /// 接入一个会话（一次连接；断了要不要重新接上由调用方定，见 Attachment）。first：握手以后的头一帧，进启动器时是 LAUNCH（launch）
    func link(_ peer: Peer, sid: String, cols: Int, rows: Int, readOnly: Bool, first: Bytes? = nil, queue: DispatchQueue,
              events: Link.Events) async throws -> Link {
        guard Frame.validSize(cols, rows), let p = store.secret(pKey(peer.pid)) else { throw RemoteFailure("error") }
        let req = try requester(peer.server)
        let t = try await req.post("relay_attach", ["sid": sid, "pid": peer.pid])
        guard let tid = t["tid"] as? String, tid.count == 32, Bytes.unhex(tid) != nil, let ts = t["tkey"] as? String,
              let tkey = Bytes.unb64(ts), tkey.count == 32, let port = jsonInt(t["port"]), (1...65535).contains(port),
              let channel = Channel(p: p, pid: peer.pid, sid: sid, fpH: peer.fp, fpV: req.identity.fp)
        else { throw RemoteFailure("error") }
        let link = Link(host: peer.server.host, port: UInt16(port), tid: tid, tkey: tkey, channel: channel, cols: cols, rows: rows,
                        readOnly: readOnly, first: first, queue: queue, events: events)
        link.start()
        return link
    }

    /// 请那台机器上的启动器照模板起一个新的会话（Launch.swift）：进启动器（票据、开场白、握手都和接入会话一样），头一帧说要起哪个
    /// （LAUNCH），等它回话（LAUNCHED），从要票据算起最多 20 秒。cols、rows：新会话的大小（LaunchAsk 的范围）；line：开场白（模板
    /// 能带的才给）；account：挑的账号（LaunchAccount.id；那台电脑上的 cca 是旧版时不给）。起来了回新会话的编号：接着用 Attachment
    /// （fresh）接入它
    public func launch(_ peer: Peer, launcher: String, template: LaunchTemplate, cols: Int, rows: Int, line: String? = nil,
                       account: String? = nil) async -> LaunchResult {
        guard let ask = LaunchAsk.frame(id: template.id, digest: template.digest, cols: cols, rows: rows, line: line, account: account) else {
            return .failed(RemoteFailure("bad_ask"))
        }
        return await awaitLaunch { events in
            try await self.link(peer, sid: launcher, cols: cols, rows: rows, readOnly: false, first: ask,
                                queue: DispatchQueue(label: "cca.remote.launch"), events: events)
        }
    }
}

/// push_set 的结果（`RemoteClient.pushSet`）
public enum PushSetResult: Equatable, Sendable {
    /// 登记上了。filtered：服务器认了 sids、只推那几个会话；没带 sids 时总是 false，旧的服务器不认 sids（照旧都推）时也是 false。
    /// usage：服务器认了 usage（推得了额度的通知）；没带 usage 时总是 false，旧的服务器（3.7.5 以前的）也是 false
    case ok(filtered: Bool, usage: Bool)
    case failed(RemoteFailure)
}
