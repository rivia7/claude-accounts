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

/// 认识的终端：App 看这台 Mac 上装了哪些（没装的在选择列表里灰着），用户挑一个当默认的。
struct TerminalApp: Identifiable, Hashable, Sendable {
    enum Launch: Hashable, Sendable {
        /// 全自动：把 .command 文件交给它，它自己开窗口运行（声明了能打开这种文件，或者收到可执行文件时直接运行）
        case openFile
        /// 全自动：开一个新进程，参数里带上脚本（`scriptToken` 换成脚本的路径）。打开文件这条路不好用的终端：
        /// Ghostty 打开文件每次都要确认，kitty 默认也是，Alacritty、Rio、Tabby 的命令行才能指定运行什么
        case arguments([String])
        /// 半自动：它没有从外面运行命令的办法。打开它，把命令放进剪贴板，用户在它的终端里粘贴
        case paste
    }

    /// bundle id
    let id: String
    let name: String
    let launch: Launch
    /// 选它时在设置里提醒一句（比如运行前它自己要确认）
    var note: String?

    /// 全自动：不用用户再粘贴。
    var isAutomatic: Bool { launch != .paste }

    static let scriptToken = "{script}"
    static let system = TerminalApp(id: "com.apple.Terminal", name: "系统终端", launch: .openFile)
    /// 认识的终端：系统自带的排最前面，然后是全自动的第三方终端（没选过时用这里面第一个装了的），半自动的在最后。
    static let known: [TerminalApp] = [
        system,
        TerminalApp(id: "com.googlecode.iterm2", name: "iTerm2", launch: .openFile, note: "第一次运行时 iTerm2 会问要不要运行这个脚本，选不再询问就行。"),
        TerminalApp(id: "com.mitchellh.ghostty", name: "Ghostty",
                    launch: .arguments(["--quit-after-last-window-closed=true", "-e", "/bin/bash", scriptToken])),
        TerminalApp(id: "dev.warp.Warp-Stable", name: "Warp", launch: .openFile),
        TerminalApp(id: "com.github.wez.wezterm", name: "WezTerm", launch: .openFile),
        TerminalApp(id: "net.kovidgoyal.kitty", name: "kitty", launch: .arguments(["--single-instance", "/bin/bash", scriptToken])),
        TerminalApp(id: "org.alacritty", name: "Alacritty", launch: .arguments(["-e", "/bin/bash", scriptToken])),
        TerminalApp(id: "org.tabby", name: "Tabby", launch: .arguments(["run", "/bin/bash", scriptToken]), note: "每次运行前 Tabby 都会弹窗确认，30 秒内点 Run。"),
        TerminalApp(id: "com.raphaelamorim.rio", name: "Rio", launch: .arguments(["-e", "/bin/bash", scriptToken])),
        TerminalApp(id: "co.zeit.hyper", name: "Hyper", launch: .openFile),
        TerminalApp(id: "com.cmuxterm.app", name: "cmux", launch: .openFile),
        TerminalApp(id: "com.termius-dmg.mac", name: "Termius", launch: .paste),
        TerminalApp(id: "dev.commandline.waveterm", name: "Wave", launch: .paste),
    ]
}

/// 要在终端里运行的一条 cca 命令（PROTOCOL 6）：`terminal` 事件，或者 App 里“添加账号”之类的入口。
struct Handoff: Sendable, Equatable, Hashable, Identifiable {
    /// `cca` 后面的参数。
    var arguments: [String]
    /// 给人看、能直接复制进终端的完整命令（`terminal` 事件里 cca 给的 display 原样用，没给就按参数拼）。
    var display: String
    var title: String
    var message: String
    /// 在哪个文件夹里运行（账号的会话）；nil 是不换目录。
    var directory: String?
    /// 命令结束后留在 shell 里（账号的会话），而不是等按键关窗。
    var staysOpen = false
    /// 一定用系统自带的“终端”（选的终端没运行起来，用户点了改用它）。
    var systemTerminal = false

    var id: String { arguments.joined(separator: "\u{1F}") }

    /// 半自动的终端：粘贴进去运行的那一行（cca 用绝对路径，不依赖那个终端的 PATH）。
    func pasteCommand(ccaPath: String) -> String {
        let run = ([ccaPath] + arguments).map(shellQuoteIfNeeded).joined(separator: " ")
        return directory.map { "cd \(shellQuoteIfNeeded($0)) && \(run)" } ?? run
    }

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

    /// 在终端里用这个账号（`cca @账号`），从 directory 启动。
    static func session(prefix: [String], key: String, name: String, directory: String) -> Handoff {
        var h = Handoff(arguments: prefix + ["@" + key], title: "在终端里打开 \(name)")
        h.directory = directory
        h.staysOpen = true
        return h
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

    /// `.command` 文件的内容：bash；补好 PATH；去掉会让 cca 自作主张的变量；进要求的文件夹；逐个加了单引号运行命令；
    /// 结束时等按任意键（账号的会话留在 shell 里，这个窗口接着能用）。文件一开始就挪走再删掉（bash 已经打开了它，
    /// 照样往下读；这样中途关窗口也不会留下文件）。挪走这一步和 App 的 `reclaim` 抢同一个 rename：App 收回了就不运行，
    /// 不会换了终端又跑一遍。
    static func script(_ h: Handoff, ccaPath: String) -> String {
        let command = ([ccaPath] + h.arguments).map(shellQuote).joined(separator: " ")
        let enter = h.directory.map { dir in
            "cd -- \(shellQuote(dir)) || { echo \(shellQuote("打不开这个文件夹：" + dir)); read -r -s -n 1 -p \"按任意键关闭…\" _; exit 1; }\n"
        } ?? ""
        let finish = h.staysOpen ? """
        if [ "$status" -ne 0 ]; then echo "命令结束（退出码 $status）。"; fi
        exec "${SHELL:-/bin/zsh}" -l

        """ : """
        if [ "$status" -eq 0 ]; then echo "完成。回到 cca App 会自动刷新。"; else echo "命令结束（退出码 $status）。"; fi
        if [ -t 0 ]; then read -r -s -n 1 -p "按任意键关闭…" _; echo; fi
        exit "$status"

        """
        return """
        #!/bin/bash
        # cca App 的终端交接：运行一次就删掉
        mv -- "$0" "$0.run" 2>/dev/null || exit 0
        rm -f -- "$0.run"
        export PATH="${PATH:-/usr/bin:/bin:/usr/sbin:/sbin}:$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin"
        unset CCA_TTY CCA_YES CCA_GRANT CCA_EVENTS CCA_RO CCA_SKIP_CHECK CCA_GUI
        \(enter)printf '%s\\n\\n' \(shellQuote("$ " + h.display))
        \(command)
        status=$?
        echo
        \(finish)
        """
    }

    /// 收回还没运行的交接脚本（选的终端没运行它）：true 表示收回了，它不会再被运行；false 表示已经开始运行了（或者已经没了）。
    static func reclaim(_ url: URL) -> Bool {
        let taken = url.appendingPathExtension("cancelled")
        guard rename(url.path, taken.path) == 0 else { return false }
        try? FileManager.default.removeItem(at: taken)
        return true
    }

    /// 写好 `.command` 文件（755），返回它的位置；打开它（用选好的终端）由 App 做。
    static func write(_ handoff: Handoff, ccaPath: String, directory: URL = defaultDirectory()) throws -> URL {
        let fm = FileManager.default
        try fm.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        cleanup(directory: directory)
        let url = directory.appendingPathComponent("cca-\(UUID().uuidString.prefix(8)).command")
        let text = script(handoff, ccaPath: ccaPath)
        try Data(text.utf8).write(to: url, options: .atomic)
        try fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path)
        return url
    }

    /// 删掉放了很久的文件（交接文件：“终端”没打开它；运行日志、事件文件：App 中途退出留下的）。
    static func cleanup(directory: URL, olderThan age: TimeInterval = 3600, now: Date = Date(),
                        extensions: Set<String> = ["command", "run"]) {
        let fm = FileManager.default
        guard let items = try? fm.contentsOfDirectory(at: directory, includingPropertiesForKeys: [.contentModificationDateKey]) else { return }
        for u in items where extensions.contains(u.pathExtension) {
            let m = (try? u.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate ?? now
            if now.timeIntervalSince(m) > age { try? fm.removeItem(at: u) }
        }
    }
}
