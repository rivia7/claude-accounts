import Foundation

/// 找 cca（PROTOCOL 1）：开发模式下 `cca gui` 用 `open --env CCA_GUI_CCA=<路径>` 告诉 App 用哪个，优先用它；
/// 否则 `~/.local/bin/cca`，没有就 `~/.cca/bin/cca`。只看文件在不在、能不能执行，不读 ~/.cca 里的任何内容。
enum CcaLocator {
    static func candidates(environment: [String: String], home: String) -> [String] {
        var out: [String] = []
        if let p = environment["CCA_GUI_CCA"], !p.isEmpty {
            out.append(expandTilde(p, home: home))
        }
        out.append((home as NSString).appendingPathComponent(".local/bin/cca"))
        out.append((home as NSString).appendingPathComponent(".cca/bin/cca"))
        return out
    }

    static func locate(environment: [String: String] = ProcessInfo.processInfo.environment,
                       home: String = NSHomeDirectory(),
                       isExecutable: (String) -> Bool = CcaLocator.isExecutableFile) -> String? {
        candidates(environment: environment, home: home).first(where: isExecutable)
    }

    /// 可执行的普通文件（或指向它的链接），不是目录。
    static func isExecutableFile(_ p: String) -> Bool {
        var dir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: p, isDirectory: &dir), !dir.boolValue else { return false }
        return FileManager.default.isExecutableFile(atPath: p)
    }

    static func expandTilde(_ p: String, home: String) -> String {
        if p == "~" { return home }
        if p.hasPrefix("~/") { return (home as NSString).appendingPathComponent(String(p.dropFirst(2))) }
        return p
    }
}
