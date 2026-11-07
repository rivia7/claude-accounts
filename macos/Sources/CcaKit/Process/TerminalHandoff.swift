import Foundation

/// 单引号转义：结果在 sh / bash 里原样是这个字符串（`it's` → `'it'\''s'`，空字符串 → `''`）。
func shellQuote(_ s: String) -> String {
    "'" + s.replacingOccurrences(of: "'", with: "'\\''") + "'"
}

/// 只有需要时才加引号（给人看、给人复制的命令）。
func shellQuoteIfNeeded(_ s: String) -> String {
    let safe = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789@%+=:,./_-")
    if !s.isEmpty, s.unicodeScalars.allSatisfy({ safe.contains($0) }) { return s }
    return shellQuote(s)
}

/// 给人看的完整命令：`cca login '工作(bob@example.com)'`。
func displayCommand(_ arguments: [String]) -> String {
    (["cca"] + arguments.map(shellQuoteIfNeeded)).joined(separator: " ")
}

/// 要在“终端”里运行的一条 cca 命令（PROTOCOL 6）：`terminal` 事件，或者 App 里“添加账号”之类的入口。
struct Handoff: Sendable, Equatable, Hashable, Identifiable {
    /// `cca` 后面的参数。
    var arguments: [String]
    /// 给人看、能直接复制进终端的完整命令（`terminal` 事件里 cca 给的 display 原样用，没给就按参数拼）。
    var display: String
    var title: String
    var message: String

    var id: String { arguments.joined(separator: "\u{1F}") }

    init(arguments: [String], display: String? = nil, title: String, message: String = "") {
        self.arguments = arguments
        self.display = display ?? displayCommand(arguments)
        self.title = title
        self.message = message
    }

    init(event: TerminalEvent) {
        self.init(arguments: event.argv, display: event.display, title: "需要在终端里完成", message: event.message)
    }

    static func login(prefix: [String], key: String, name: String) -> Handoff {
        Handoff(arguments: prefix + ["login", key], title: "登录 \(name)", message: "在终端里登录这个账号，完成后回到 cca。")
    }

    static func edit(prefix: [String]) -> Handoff {
        Handoff(arguments: prefix + ["edit"], title: "编辑账号表", message: "在终端里用编辑器修改账号表，保存后 cca 会检查格式。")
    }

    static func addAccount(prefix: [String], product: String) -> Handoff {
        Handoff(arguments: prefix + ["add"], title: "添加 \(product) 账号", message: "在终端里按提示添加账号并登录。")
    }

    static let addAPI = Handoff(arguments: ["add", "api"], title: "添加第三方 API 账号",
                                message: "在终端里填写接口地址、模型和 key（key 存进钥匙串，不经过 cca App）。")
    static let routerSetup = Handoff(arguments: ["router", "setup"], title: "配置路由器", message: "在终端里按提示配置路由器。")
    static let localSetup = Handoff(arguments: ["local", "setup"], title: "配置本机 Clash", message: "在终端里按提示配置本机的 Clash。")
    static let install = Handoff(arguments: ["install"], title: "安装 cca", message: "在终端里完成 cca 的安装。")
    static let update = Handoff(arguments: ["update"], title: "更新 cca", message: "在终端里把 cca 更新到最新版本。")
}

enum TerminalHandoff {
    /// `~/Library/Caches/local.cca.app/handoff/`
    static func defaultDirectory() -> URL {
        let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("Library/Caches")
        return caches.appendingPathComponent("local.cca.app/handoff", isDirectory: true)
    }

    /// `.command` 文件的内容：bash；补好 PATH；去掉会让 cca 自作主张的变量；逐个加了单引号运行命令；
    /// 结束时等按任意键。文件一开始就删掉自己（bash 已经打开了它，照样往下读；这样中途关窗口也不会留下文件）。
    static func script(ccaPath: String, arguments: [String], display: String) -> String {
        let command = ([ccaPath] + arguments).map(shellQuote).joined(separator: " ")
        return """
        #!/bin/bash
        # cca App 的终端交接：运行一次就删掉
        rm -f -- "$0"
        export PATH="${PATH:-/usr/bin:/bin:/usr/sbin:/sbin}:$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin"
        unset CCA_TTY CCA_YES CCA_GRANT CCA_EVENTS CCA_RO CCA_SKIP_CHECK CCA_GUI
        printf '%s\\n\\n' \(shellQuote("$ " + display))
        \(command)
        status=$?
        echo
        if [ "$status" -eq 0 ]; then echo "完成。回到 cca App 会自动刷新。"; else echo "命令结束（退出码 $status）。"; fi
        if [ -t 0 ]; then read -r -s -n 1 -p "按任意键关闭…" _; echo; fi
        exit "$status"

        """
    }

    /// 写好 `.command` 文件（755），返回它的位置；打开它（用“终端”）由 App 做。
    static func write(_ handoff: Handoff, ccaPath: String, directory: URL = defaultDirectory()) throws -> URL {
        let fm = FileManager.default
        try fm.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        cleanup(directory: directory)
        let url = directory.appendingPathComponent("cca-\(UUID().uuidString.prefix(8)).command")
        let text = script(ccaPath: ccaPath, arguments: handoff.arguments, display: handoff.display)
        try Data(text.utf8).write(to: url, options: .atomic)
        try fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path)
        return url
    }

    /// 删掉放了很久的文件（交接文件：“终端”没打开它；运行日志、事件文件：App 中途退出留下的）。
    static func cleanup(directory: URL, olderThan age: TimeInterval = 3600, now: Date = Date(),
                        extensions: Set<String> = ["command"]) {
        let fm = FileManager.default
        guard let items = try? fm.contentsOfDirectory(at: directory, includingPropertiesForKeys: [.contentModificationDateKey]) else { return }
        for u in items where extensions.contains(u.pathExtension) {
            let m = (try? u.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate ?? now
            if now.timeIntervalSince(m) > age { try? fm.removeItem(at: u) }
        }
    }
}
