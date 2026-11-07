import Foundation

/// 子进程的环境（PROTOCOL 1）：继承 App 的环境，去掉会让 cca 以为有人在终端里回答、或者跳过检查的变量；
/// PATH 后面补上常见的安装位置；设 CCA_GUI=1、NO_COLOR=1；按需设 CCA_EVENTS、CCA_GRANT。
enum CcaEnvironment {
    /// 一律去掉。CCA_SKIP_CHECK 不在 PROTOCOL 的去掉列表里，但规则是“绝不设”，继承来的也不能带过去。
    static let removed: Set<String> = ["CCA_TTY", "CCA_YES", "CCA_GRANT", "CCA_EVENTS", "CCA_RO", "CCA_SKIP_CHECK"]
    /// 只有 App 自己能设的变量（调用方传进来的 extra 里也不收）。
    static let forbidden: Set<String> = ["CCA_TTY", "CCA_YES", "CCA_SKIP_CHECK", "CCA_RO"]

    static func pathExtras(home: String) -> [String] {
        [(home as NSString).appendingPathComponent(".local/bin"), "/opt/homebrew/bin", "/usr/local/bin"]
    }

    static func make(base: [String: String], home: String, eventsPath: String? = nil, grants: [String] = [],
                     extra: [String: String] = [:]) -> [String: String] {
        var env = base
        for k in removed { env.removeValue(forKey: k) }
        var parts = (env["PATH"] ?? "").split(separator: ":", omittingEmptySubsequences: true).map(String.init)
        if parts.isEmpty { parts = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] }
        for p in pathExtras(home: home) where !parts.contains(p) { parts.append(p) }
        env["PATH"] = parts.joined(separator: ":")
        if env["HOME"] == nil { env["HOME"] = home }
        env["CCA_GUI"] = "1"
        env["NO_COLOR"] = "1"
        for (k, v) in extra where !forbidden.contains(k) && !removed.contains(k) {
            env[k] = v
        }
        if let eventsPath { env["CCA_EVENTS"] = eventsPath }
        let g = grants.filter { !$0.isEmpty }
        if !g.isEmpty { env["CCA_GRANT"] = g.joined(separator: ",") }
        return env
    }
}
