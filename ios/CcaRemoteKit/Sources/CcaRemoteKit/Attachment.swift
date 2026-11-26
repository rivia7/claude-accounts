// 接入一个会话：一条连接从开场白、握手到收发（Link），断了照辅助程序的办法重新接上（Attachment）。
// 常量和 cli/src/cmd/remote/attach.rs、server/web/core.js 的同名常量一样。
import Foundation

/// 一处接入：一条 TCP 连接进数据端口。事件在 queue 上出来。关掉的原因是一个词：
/// detached（自己断开的）、ended（会话结束了）、lost（连接断了）、unreachable（连不上数据端口）、refused（宿主不认这个配对）、
/// version、auth（对面证明不了它有密钥：什么都没发给它）、nohost（那台机器这会儿没连着服务器）、busy、ticket、error
final class Link: @unchecked Sendable {
    struct Events {
        var open: () -> Void = {}
        var output: (Bytes) -> Void = { _ in }
        var exit: (Int32) -> Void = { _ in }
        var note: (Note) -> Void = { _ in }
        var close: (String) -> Void = { _ in }
    }

    private enum Phase { case line, reply, open }

    private let queue: DispatchQueue
    private let stream: TCPStream
    private let channel: Channel
    private let line: String
    private let expect: String
    private var events: Events
    private var phase = Phase.line
    private var buf = Bytes()
    private var plain = Bytes()
    private var done = false
    private var cols: Int
    private var rows: Int
    private let readOnly: Bool
    private var timer: DispatchSourceTimer?
    private var beat: DispatchSourceTimer?
    /// 最后一次听到对面（秒，单调时钟）
    private(set) var heard = monotonic()

    init(host: String, port: UInt16, tid: String, tkey: Bytes, channel: Channel, cols: Int, rows: Int, readOnly: Bool,
         queue: DispatchQueue, events: Events) {
        self.queue = queue
        self.channel = channel
        self.events = events
        self.cols = cols
        self.rows = rows
        self.readOnly = readOnly
        (line, expect) = relayLine(tid: tid, tkey: tkey)
        stream = TCPStream(host: host, port: port, queue: queue)
    }

    func start() {
        queue.async { [self] in
            stream.onReady = { [weak self] in
                guard let self else { return }
                self.stream.send(Bytes(utf8: self.line))
            }
            stream.onData = { [weak self] b in self?.guarded { try self?.feed(b) } }
            stream.onClose = { [weak self] in
                guard let self else { return }
                self.finish(self.phase == .line ? "unreachable" : "lost")
            }
            stream.start()
            // 等宿主那头连上来（服务器最多等它 10 秒）、握手：太久就算了
            timer = after(25) { [weak self] in
                guard let self else { return }
                self.finish(self.phase == .line ? "nohost" : "lost")
            }
        }
    }

    private func after(_ secs: Double, repeating: Double? = nil, _ f: @escaping () -> Void) -> DispatchSourceTimer {
        let t = DispatchSource.makeTimerSource(queue: queue)
        if let repeating {
            t.schedule(deadline: .now() + secs, repeating: repeating, leeway: .seconds(1))
        } else {
            t.schedule(deadline: .now() + secs, leeway: .milliseconds(200))
        }
        t.setEventHandler(handler: f)
        t.resume()
        return t
    }

    private struct Failed: Error {
        let why: String
    }

    private func guarded(_ f: () throws -> Void) {
        do {
            try f()
        } catch let e as Failed {
            finish(e.why)
        } catch {
            finish("error")
        }
    }

    private func feed(_ data: Bytes) throws {
        guard !done else { return }
        heard = monotonic()
        buf += data
        if phase == .line {
            guard let nl = buf.firstIndex(of: 10) else {
                if buf.count > 200 { throw Failed(why: "error") }
                return
            }
            let got = String(decoding: buf[..<nl], as: UTF8.self).trimmingCharacters(in: .whitespaces)
            buf.removeFirst(nl + 1)
            guard got == expect else {
                // 服务器只会说这几个词；别的不原样往外传
                let words = ["ticket": "ticket", "bad": "ticket", "nohost": "nohost", "taken": "ticket", "sessions": "busy", "slow": "ticket"]
                let w = got.hasPrefix("CCA-RELAY/1 ERR ") ? words[String(got.dropFirst(16))] : nil
                throw Failed(why: w ?? "error")
            }
            phase = .reply
            stream.send(channel.hello())
        }
        if phase == .reply {
            // 宿主不认这个配对、版本不一样：回 4 个字节 "CR" 7F 原因 就断开
            if buf.count >= 4, buf[2] == 0x7F { throw Failed(why: buf[3] == 4 ? "version" : "refused") }
            guard buf.count >= 68 else { return }
            guard let confirm = channel.confirm(Bytes(buf.prefix(68))) else { throw Failed(why: "auth") }
            buf.removeFirst(68)
            stream.send(confirm)
            phase = .open
            timer?.cancel()
            timer = nil
            try write(Frame.make(Frame.hello, [1, readOnly ? 1 : 0] + Frame.size(cols, rows)))
            // 两头每 20 秒说一声（空的记录），60 秒没听到对面就算断了
            beat = after(20, repeating: 20) { [weak self] in
                guard let self else { return }
                if monotonic() - self.heard > 60 {
                    self.finish("lost")
                } else {
                    self.guarded { try self.stream.send(self.channel.seal([])) }
                }
            }
            events.open()
        }
        if phase == .open {
            try records()
        }
    }

    private func records() throws {
        var at = 0
        while let rec = try channel.open(buf[at...]) {
            at += rec.used
            plain += rec.data
        }
        buf.removeFirst(at)
        while !done, plain.count >= 5 {
            let n = readU32(plain[1..<5])
            if n > Frame.max { throw Failed(why: "error") }
            guard plain.count >= 5 + n else { break }
            let kind = plain[0]
            let payload = Bytes(plain[5..<5 + n])
            plain.removeFirst(5 + n)
            if kind == Frame.output {
                events.output(payload)
            } else if kind == Frame.exit {
                let code = payload.count >= 4 ? Int32(bitPattern: UInt32(readU32(payload[0..<4]))) : 0
                events.exit(code)
                finish("ended")
            } else if kind == Frame.notify {
                // 会话推来的通知：解不开、什么字都没有的不算
                if let n = Note.parse(payload) { events.note(n) }
            }
            // 别的帧（宿主那头已经滤掉了）不理
        }
    }

    /// 帧的字节切成记录发出去
    private func write(_ bytes: Bytes) throws {
        guard phase == .open, !done else { return }
        var at = 0
        while at < bytes.count {
            let end = Swift.min(at + Channel.recordMax, bytes.count)
            stream.send(try channel.seal(Bytes(bytes[at..<end])))
            at = end
        }
    }

    func input(_ data: Bytes) {
        queue.async { [self] in
            guard !readOnly, !data.isEmpty else { return }
            guarded {
                var at = 0
                while at < data.count {
                    let end = Swift.min(at + 8192, data.count)
                    try write(Frame.make(Frame.input, Bytes(data[at..<end])))
                    at = end
                }
            }
        }
    }

    func resize(_ cols: Int, _ rows: Int) {
        queue.async { [self] in
            guard Frame.validSize(cols, rows), cols != self.cols || rows != self.rows else { return }
            self.cols = cols
            self.rows = rows
            guarded { try write(Frame.make(Frame.resize, Frame.size(cols, rows))) }
        }
    }

    /// 自己断开（会话接着运行）
    func detach() {
        queue.async { [self] in
            guarded { try write(Frame.make(Frame.detach, [])) }
            finish("detached")
        }
    }

    /// 当成连接断了（外面看出来的：App 挂起过，好一阵没听到对面）
    func lost() {
        queue.async { self.finish("lost") }
    }

    /// 在 queue 上调
    func finish(_ why: String) {
        guard !done else { return }
        done = true
        timer?.cancel()
        beat?.cancel()
        stream.onClose = nil
        // 刚交给连接的（DETACH）先发出去再断：关得太快对面收不到
        let s = stream
        queue.asyncAfter(deadline: .now() + 0.2) { s.close() }
        let close = events.close
        events = Events()
        close(why)
    }
}

func monotonic() -> Double {
    Double(DispatchTime.now().uptimeNanoseconds) / 1e9
}

/// 一个会话在这台设备上的接入：断了（网络断了、App 被切到后台、服务器重启）照辅助程序的办法重新接上。
/// 事件在主线程上出来。
public final class Attachment: @unchecked Sendable {
    public enum State: Equatable, Sendable {
        case connecting, open, retrying
        /// 不再接了。原因是 Link 的那几个词，再加 gone（会话不在了）、unpaired（配对没了）、gave_up（一直没接上）
        case closed(String)
    }

    static let retryGap: [Double] = [1, 2, 4, 8, 15]   // 秒；之后一直 15
    static let retryFor = 300.0                        // 一共试这么久
    static let missingFor = 45.0                       // 服务器说没有这个会话：多半是结束了，再等这么久就不等了
    static let flapsMax = 5                            // 接上不到 10 秒又断，连着这么多次就不再试

    public let peer: Peer
    public let sid: String
    public let readOnly: Bool
    private let client: RemoteClient
    private let queue = DispatchQueue(label: "cca.remote.attachment")
    private var cols: Int
    private var rows: Int
    private var link: Link?
    private var attempt = 0
    private var flaps = 0
    private var tries = 0
    private var openedAt = 0.0
    private var since: Double?
    private var missing: Double?
    private var timer: DispatchWorkItem?
    private var stateNow = State.connecting

    /// 会话推来的通知限流（几次重新接上算一处，和辅助程序的接入端一样）
    private var notes = NoteBudget()

    /// 终端的字节、状态、退出码、会话推来的通知（主线程上）
    public var onOutput: (Bytes) -> Void = { _ in }
    public var onState: (State) -> Void = { _ in }
    public var onExit: (Int32) -> Void = { _ in }
    public var onNote: (Note) -> Void = { _ in }

    public init(client: RemoteClient, peer: Peer, sid: String, cols: Int, rows: Int, readOnly: Bool) {
        self.client = client
        self.peer = peer
        self.sid = sid
        self.cols = cols
        self.rows = rows
        self.readOnly = readOnly
    }

    /// 开始接（回调都设好以后调一次）
    public func start() {
        queue.async { self.connect(retrying: false) }
    }

    public var state: State {
        queue.sync { stateNow }
    }

    private func set(_ s: State) {
        stateNow = s
        DispatchQueue.main.async { self.onState(s) }
    }

    private func connect(retrying: Bool) {
        attempt += 1
        let me = attempt
        let events = Link.Events(
            open: { [weak self] in self?.queue.async { self?.opened(me) } },
            output: { [weak self] b in
                guard let self else { return }
                self.queue.async {
                    guard self.attempt == me else { return }
                    DispatchQueue.main.async { self.onOutput(b) }
                }
            },
            exit: { [weak self] code in
                guard let self else { return }
                self.queue.async {
                    guard self.attempt == me else { return }
                    DispatchQueue.main.async { self.onExit(code) }
                }
            },
            note: { [weak self] n in
                guard let self else { return }
                self.queue.async {
                    guard self.attempt == me, self.notes.take() else { return }
                    DispatchQueue.main.async { self.onNote(n) }
                }
            },
            close: { [weak self] why in self?.queue.async { self?.closed(me, why) } })
        let (cols, rows) = (self.cols, self.rows)
        Task {
            do {
                let l = try await client.link(peer, sid: sid, cols: cols, rows: rows, readOnly: readOnly,
                                              queue: DispatchQueue(label: "cca.remote.link"), events: events)
                queue.async {
                    if self.attempt != me {
                        l.detach()
                    } else {
                        self.link = l
                    }
                }
            } catch {
                let code = (error as? RemoteFailure)?.code ?? "error"
                queue.async { self.failed(me, code, retrying: retrying) }
            }
        }
    }

    private func failed(_ me: Int, _ code: String, retrying: Bool) {
        guard attempt == me else { return }
        if code == "no_session" {
            // 第一次就没有：会话不在了。重新接的时候：多半是结束了，也可能是那头正在重连，留一阵
            if !retrying { return stop("gone") }
            missing = missing ?? monotonic()
            if monotonic() - (missing ?? 0) > Self.missingFor { return stop("gone") }
            return later()
        }
        if code == "denied" || code == "unknown_device" { return stop("unpaired") }
        if !retrying, !["unreachable", "rate", "busy"].contains(code) { return stop(code) }
        missing = nil
        later()
    }

    private func opened(_ me: Int) {
        guard attempt == me else { return }
        openedAt = monotonic()
        since = nil
        missing = nil
        set(.open)
    }

    private func closed(_ me: Int, _ why: String) {
        guard attempt == me else { return }
        link = nil
        if ["detached", "ended", "refused", "version", "auth"].contains(why) { return stop(why) }
        if stateNow == .open {
            flaps = monotonic() - openedAt < 10 ? flaps + 1 : 0
            if flaps >= Self.flapsMax { return stop("gave_up") }
            tries = flaps   // 刚接上就断的：间隔从长一点的起步，别一秒一次地问服务器
        }
        later()
    }

    private func later() {
        since = since ?? monotonic()
        if monotonic() - (since ?? 0) > Self.retryFor { return stop("gave_up") }
        let gap = Self.retryGap[Swift.min(tries, Self.retryGap.count - 1)]
        tries += 1
        set(.retrying)
        let me = attempt
        let w = DispatchWorkItem { [weak self] in
            guard let self, self.attempt == me else { return }
            self.connect(retrying: true)
        }
        timer = w
        queue.asyncAfter(deadline: .now() + gap, execute: w)
    }

    private func stop(_ why: String) {
        attempt += 1
        timer?.cancel()
        set(.closed(why))
    }

    /// App 回到前台：断着的马上再试，不等退避的那几秒；开着却好一阵没听到对面的（手机把 App 挂起过，连接多半已经死了）
    /// 当成断了，不等 60 秒的那次检查
    public func nudge() {
        queue.async {
            if self.stateNow == .open, let l = self.link, monotonic() - l.heard > 25 {
                l.lost()   // 它的 close 回来走 closed：和真的断了一样重新接
                return
            }
            guard self.stateNow == .retrying else { return }
            self.timer?.cancel()
            self.connect(retrying: true)
        }
    }

    /// 敲的键、粘贴的字。回：送出去了没有（断着、还没接上时不送，也不攒着等接上——迟到的按键比丢了的更糟）
    @discardableResult
    public func input(_ data: Bytes) -> Bool {
        queue.sync {
            guard stateNow == .open, let link else { return false }
            link.input(data)
            return true
        }
    }

    public func resize(cols: Int, rows: Int) {
        queue.async {
            guard Frame.validSize(cols, rows) else { return }
            self.cols = cols
            self.rows = rows
            self.link?.resize(cols, rows)
        }
    }

    /// 自己断开（会话接着运行）
    public func detach() {
        queue.async {
            if case .closed = self.stateNow { return }
            let l = self.link
            self.link = nil
            self.attempt += 1
            self.timer?.cancel()
            l?.detach()
            self.set(.closed("detached"))
        }
    }
}
