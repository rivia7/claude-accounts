import Foundation

// cca App 的契约测试（没有测试框架：check(名字, 条件) 打印 PASS / FAIL，有失败就非 0 退出）。
//
//   contract-tests --full 目录      手写的 Tests/Fixtures：通用不变量 + 针对这些数据的断言 + 各部件的单元测试
//   contract-tests --generic 目录   回归测试（tests/regress.sh 10g）生成的真实 cca 输出：只解码 + 通用不变量 + 枚举值都认识，
//                                   不认具体的账号、数量、名字；缺的文件跳过
//
// 临时文件一律放在 CCA_TEST_TMP（test.sh 设成 macos/build/tmp）下面。

// MARK: - 测试框架

final class Harness: @unchecked Sendable {
    private let lock = NSLock()
    private(set) var passed = 0
    private(set) var failed = 0
    var prefix = ""

    func record(_ name: String, _ ok: Bool, _ detail: String) {
        lock.lock()
        defer { lock.unlock() }
        if ok {
            passed += 1
            print("PASS \(prefix)\(name)")
        } else {
            failed += 1
            print("FAIL \(prefix)\(name)\(detail.isEmpty ? "" : " — \(detail)")")
        }
    }
}

let H = Harness()

func check(_ name: String, _ cond: Bool, _ detail: @autoclosure () -> String = "") {
    H.record(name, cond, cond ? "" : detail())
}

func section(_ s: String) {
    print("== \(s)")
    H.prefix = s + " · "
}

let cli = Array(CommandLine.arguments.dropFirst())
let fullMode = cli.first == "--full"
let fixtureDir = URL(fileURLWithPath: cli.count >= 2 ? cli[1] : "Tests/Fixtures", isDirectory: true)
let tmpRoot: URL = {
    let base = ProcessInfo.processInfo.environment["CCA_TEST_TMP"] ?? NSTemporaryDirectory()
    let u = URL(fileURLWithPath: base, isDirectory: true).appendingPathComponent("contract-\(getpid())", isDirectory: true)
    try? FileManager.default.createDirectory(at: u, withIntermediateDirectories: true)
    return u
}()

func fixture(_ name: String) -> Data? {
    try? Data(contentsOf: fixtureDir.appendingPathComponent(name))
}

func jsonObject(_ d: Data) -> Any? {
    try? JSONSerialization.jsonObject(with: d, options: [.fragmentsAllowed])
}

func isBool(_ x: Any?) -> Bool {
    guard let n = x as? NSNumber else { return false }
    return CFGetTypeID(n) == CFBooleanGetTypeID()
}

func isNumber(_ x: Any?) -> Bool {
    guard let n = x as? NSNumber else { return false }
    return CFGetTypeID(n) != CFBooleanGetTypeID()
}

func isNull(_ x: Any?) -> Bool { x == nil || x is NSNull }

func decodeOrFail<T: CcaDocument>(_ type: T.Type, _ name: String, _ d: Data) -> T? {
    do {
        let v = try DocumentDecoder.decode(T.self, from: d)
        check("\(name) 解码", true)
        return v
    } catch {
        check("\(name) 解码", false, "\(error)")
        return nil
    }
}

/// 所有字符串值（找像密钥的东西）。
func walkStrings(_ x: Any, _ f: (String) -> Void) {
    if let s = x as? String { f(s) }
    if let d = x as? [String: Any] { for (_, v) in d { walkStrings(v, f) } }
    if let a = x as? [Any] { for v in a { walkStrings(v, f) } }
}

func looksSecret(_ s: String) -> Bool {
    s.range(of: #"^(sk-|sk_|ghp_|xox[bp]-)[A-Za-z0-9_-]{8,}"#, options: .regularExpression) != nil
        || s.range(of: "Bearer ", options: .caseInsensitive) != nil
        || s.range(of: "BEGIN [A-Z ]*PRIVATE KEY", options: .regularExpression) != nil
}

/// 断言是失败（`.failed`），并且 Failure 满足 cond。
func expectFailure(_ name: String, _ status: ExecStatus, _ cond: (Failure) -> Bool) {
    if case .failed(let f) = status {
        check(name, cond(f), "\(f)")
    } else {
        check(name, false, "不是失败：\(status)")
    }
}
