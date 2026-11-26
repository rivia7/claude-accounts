// 远程启动（docs/dev/ios.md「新建会话（远程启动）」）：允许远程启动的手机挑那台电脑上写好的一个模板（工具、账号、文件夹、参数），
// 那台电脑起一个新的会话，手机接着接入它。那台电脑上常驻的启动器（cli/src/cmd/remote/launch.rs）在服务器上登记成一个“会话”，
// 它的说明（有哪些模板）用另一把密钥封给这台手机（launchOpen：旧的 App 拿会话说明的密钥解不开，当它不存在）。点了模板：进启动器
// （和接入会话一样的票据、开场白、握手），头一帧说要起哪个（LAUNCH，不发 HELLO），等它回话（LAUNCHED）。格式见 server/README.md
// 「启动器」，和辅助程序的 launch.rs、template.rs、crypto.rs 的 launch_seal，tests/kit/spec.rs 的 launch_seal 是同一套
import Foundation

/// 那台电脑上的一个模板（启动器的说明里的一项，清理过）
public struct LaunchTemplate: Sendable, Equatable, Hashable, Identifiable {
    /// 编号（8 个小写的十六进制字符）
    public let id: String
    /// 摘要（16 个小写的十六进制字符）：工具、账号、文件夹、参数算出来的，起会话时带回去；那台电脑上改过了就对不上（changed）
    public let digest: String
    public let name: String
    /// claude、codex、agy
    public let tool: String
    /// 账号（给人看的短名字）
    public let account: String
    /// 账号摘要（和账号的状态、会话的说明里的一样；那台电脑不在 Team 里时是空的）
    public let aid: String
    /// 文件夹（~/… 的样子）
    public let dir: String
    /// 参数（照 shell 的写法引起来、空格隔开；太长的截断了，最后是 …）
    public let args: String
    /// 能不能带开场白（会话起来以后当作头一句话）
    public let line: Bool

    public init(id: String, digest: String, name: String, tool: String, account: String, aid: String = "", dir: String, args: String,
                line: Bool) {
        self.id = id
        self.digest = digest
        self.name = name
        self.tool = tool
        self.account = account
        self.aid = aid
        self.dir = dir
        self.args = args
        self.line = line
    }

    /// 模板的工具只能是这几个
    static let tools: Set<String> = ["claude", "codex", "agy"]
}

/// 启动器的说明：那台电脑叫什么、这台手机能起的模板、说明里放不下的还有几个
public struct LauncherInfo: Sendable, Equatable {
    public let machine: String
    public let templates: [LaunchTemplate]
    public let more: Int

    public init(machine: String, templates: [LaunchTemplate], more: Int = 0) {
        self.machine = machine
        self.templates = templates
        self.more = more
    }
}

/// 一台电脑上的启动器（列会话时认出来的，PeerListing.launcher）。和 Mac App 的 `RemoteLauncher`（这台 Mac 上的后台进程在不在跑）
/// 不是一回事：名字也分开
public struct HostLauncher: Sendable, Equatable {
    /// 它在服务器上登记成的那个会话的编号：进启动器要的是它的票据
    public let sid: String
    public let info: LauncherInfo
    /// 启动器这会儿连着服务器
    public let online: Bool

    public init(sid: String, info: LauncherInfo, online: Bool) {
        self.sid = sid
        self.info = info
        self.online = online
    }
}

/// P 派生的“启动器的说明”的密钥（和会话的说明的不是同一把：互相解不开）
func launchKeys(_ p: Bytes) -> SealKeys {
    SealKeys(from: Mac(key: hmac(p, Bytes(utf8: "cca-remote/v1/launch"))))
}

/// 解开启动器的说明：认证带着配对编号、启动器的编号（别的配对的、别的密钥封的、会话的说明都解不开）；不是 v1 的是 nil。
/// 模板：编号、摘要、工具不是那个样子的不要，编号和前面留下的重复的不要（留头一个）；字照会话的说明那样清理、限长度
func launchOpen(_ keys: SealKeys, pid: String, sid: String, _ blob: Any?) -> LauncherInfo? {
    guard let pt = unseal("cca-remote-launch", keys, [pid, sid], blob), let j = parsed(pt), jsonInt(j["v"]) == 1 else { return nil }
    var seen = Set<String>()
    var rows = [LaunchTemplate]()
    for x in j["t"] as? [Any] ?? [] {
        guard let t = x as? [String: Any], let id = t["i"] as? String, lowerHex(id, 8), let h = t["h"] as? String, lowerHex(h, 16),
              let k = t["k"] as? String, LaunchTemplate.tools.contains(k), !seen.contains(id)
        else { continue }
        seen.insert(id)
        let aid = (t["aid"] as? String).flatMap { lowerHex($0, 32) ? $0 : nil } ?? ""
        rows.append(LaunchTemplate(id: id, digest: h, name: clean(t["n"], 40), tool: k, account: clean(t["a"], 64), aid: aid,
                                   dir: clean(t["d"], 200), args: clean(t["x"], 300), line: jsonBool(t["l"]) == true))
    }
    return LauncherInfo(machine: clean(j["machine"], 48), templates: rows, more: max(jsonInt(j["more"]) ?? 0, 0))
}

/// 要起哪个（LAUNCH 帧的内容）：`{"v":1,"t":编号,"h":摘要,"c":列,"r":行,"l":开场白}`，紧凑的 JSON、非 ASCII 原样、键照这个先后，
/// 没有开场白时不带 l。和辅助程序 launch.rs 的 `Ask::parse` 是一对：它不认的这里就不发
public enum LaunchAsk {
    /// 新会话的大小（列、行）的范围
    public static let cols = 20...500
    public static let rows = 5...300
    /// 开场白最多几个字（按 Unicode 码位数，和辅助程序的 chars().count() 一样）
    public static let lineMax = 500
    /// LAUNCH 的内容最长多少个字节
    static let payloadMax = 4096

    /// 开场白能不能发：不是空白，不超过 500 个字，没有控制字符（换行、制表符也算）
    public static func lineOK(_ s: String) -> Bool {
        let u = s.unicodeScalars
        return u.count <= lineMax && !u.allSatisfy { $0.properties.isWhitespace } && !u.contains { $0.properties.generalCategory == .control }
    }

    /// 大小夹到范围里（App 上次用的、照屏幕算的）
    public static func clamp(cols c: Int, rows r: Int) -> (cols: Int, rows: Int) {
        (min(max(c, cols.lowerBound), cols.upperBound), min(max(r, rows.lowerBound), rows.upperBound))
    }

    /// LAUNCH 的内容。编号、摘要不是那个样子，大小不在范围里，开场白不能发（空的、空白、太长、有控制字符）：nil
    public static func payload(id: String, digest: String, cols c: Int, rows r: Int, line: String? = nil) -> Bytes? {
        guard lowerHex(id, 8), lowerHex(digest, 16), cols.contains(c), rows.contains(r) else { return nil }
        var s = "{\"v\":1,\"t\":\"\(id)\",\"h\":\"\(digest)\",\"c\":\(c),\"r\":\(r)"
        if let line {
            guard lineOK(line) else { return nil }
            s += ",\"l\":" + quoted(line)
        }
        let b = Bytes(utf8: s + "}")
        return b.count <= payloadMax ? b : nil
    }

    /// 整个 LAUNCH 帧（类型、4 个字节的大端长度、内容）；不能发是 nil（同 payload）
    public static func frame(id: String, digest: String, cols c: Int, rows r: Int, line: String? = nil) -> Bytes? {
        payload(id: id, digest: digest, cols: c, rows: r, line: line).map { Frame.make(Frame.launch, $0) }
    }

    /// JSON 的字符串：引号、反斜杠转义，控制字符写成 \u00XX（开场白里本来就不许有），别的原样
    private static func quoted(_ s: String) -> String {
        var out = String.UnicodeScalarView()
        out.append("\"")
        for u in s.unicodeScalars {
            switch u {
            case "\"", "\\":
                out.append("\\")
                out.append(u)
            case _ where u.value < 0x20:
                out.append(contentsOf: String(format: "\\u%04x", u.value).unicodeScalars)
            default:
                out.append(u)
            }
        }
        out.append("\"")
        return String(out)
    }
}

/// 远程启动的结果（RemoteClient.launch）
public enum LaunchResult: Equatable, Sendable {
    /// 起来了：新会话的编号（接着用 Attachment 接入它，fresh）
    case launched(sid: String)
    /// 启动器说不起：原因码（denied、changed……，App 的 Texts.launchFailure 照它说）、那台电脑写的原因（清理过，可能是空的）
    case refused(code: String, text: String)
    /// 没等到启动器回话。原因是服务器、这一头的（RemoteFailure 的那些：unreachable、no_session……）、连接关掉的那几个词（Link：
    /// nohost、lost、refused……），另有 timeout（20 秒没回话）、cancelled（调用方不等了）、bad_ask（大小、开场白不能发）
    case failed(RemoteFailure)

    /// LAUNCHED 帧的内容：`{"v":1,"ok":true,"sid":新会话的编号}` 或者 `{"v":1,"ok":false,"code":原因码,"text":原因}`；样子不对是 nil
    public static func parse(_ payload: Bytes) -> LaunchResult? {
        guard let j = parsed(payload), jsonInt(j["v"]) == 1, let ok = jsonBool(j["ok"]) else { return nil }
        if ok {
            guard let sid = j["sid"] as? String, lowerHex(sid, 32) else { return nil }
            return .launched(sid: sid)
        }
        guard let code = j["code"] as? String, (1...32).contains(code.utf8.count),
              code.utf8.allSatisfy({ (0x61...0x7A).contains($0) || $0 == 0x5F })
        else { return nil }
        return .refused(code: code, text: clean(j["text"], 200))
    }
}

/// 起一条进启动器的连接（open：要票据、起 Link，头一帧是 LAUNCH），等它回话：先到的那一个算数——它的回话、连接关了、timeout 秒到了
/// （从调用算起）、调用方的 Task 取消了（cancelled）
func awaitLaunch(timeout: Double = LaunchWait.timeout, _ open: @escaping (Link.Events) async throws -> Link) async -> LaunchResult {
    let wait = LaunchWait()
    let events = {
        var e = Link.Events()
        e.launched = { b in wait.end(LaunchResult.parse(b) ?? .failed(RemoteFailure("error"))) }
        e.close = { why in wait.end(.failed(RemoteFailure(why))) }
        return e
    }()
    return await withTaskCancellationHandler {
        await withCheckedContinuation { (cont: CheckedContinuation<LaunchResult, Never>) in
            wait.begin(cont, timeout: timeout)
            Task {
                do {
                    wait.hold(try await open(events))
                } catch {
                    wait.end(.failed(error as? RemoteFailure ?? RemoteFailure("error")))
                }
            }
        }
    } onCancel: {
        wait.end(.failed(RemoteFailure("cancelled")))
    }
}

/// 等启动器回话（awaitLaunch）：只认头一个结果，之后来的不理；到了就关掉连接、不再计时
final class LaunchWait: @unchecked Sendable {
    /// 从要票据算起，最多等这么久（秒）
    static let timeout = 20.0

    private let lock = NSLock()
    private var cont: CheckedContinuation<LaunchResult, Never>?
    private var result: LaunchResult?
    private var link: Link?
    private var timer: DispatchWorkItem?

    func begin(_ c: CheckedContinuation<LaunchResult, Never>, timeout: Double = LaunchWait.timeout) {
        let early: LaunchResult? = lock.withLock {
            if result == nil { cont = c }
            return result
        }
        if let early { return c.resume(returning: early) }
        let w = DispatchWorkItem { [weak self] in self?.end(.failed(RemoteFailure("timeout"))) }
        lock.withLock { timer = w }
        DispatchQueue.global().asyncAfter(deadline: .now() + timeout, execute: w)
    }

    /// 连接建好了：结束时关掉它（已经结束了的马上关）
    func hold(_ l: Link) {
        let over = lock.withLock { () -> Bool in
            if result == nil { link = l }
            return result != nil
        }
        if over { l.lost() }
    }

    func end(_ r: LaunchResult) {
        let taken: (CheckedContinuation<LaunchResult, Never>?, Link?, DispatchWorkItem?)? = lock.withLock {
            guard result == nil else { return nil }
            result = r
            defer { (cont, link, timer) = (nil, nil, nil) }
            return (cont, link, timer)
        }
        guard let (c, l, t) = taken else { return }
        t?.cancel()
        l?.lost()
        c?.resume(returning: r)
    }
}
