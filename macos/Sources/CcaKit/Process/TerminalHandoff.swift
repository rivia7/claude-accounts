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

/// 认识的终端：App 看这台 Mac 上装了哪些（没装的在选择列表里灰着），用户挑一个用来开账号的会话。
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
    /// 用系统自带的“终端”：App 发起的命令（登录、添加账号、配置路由器……）都是。只有账号的会话用用户选的终端；
    /// 选的终端没运行起来、用户点了改用系统自带的时，也变回这样。
    var systemTerminal = true
    /// 这条流程能在 App 里问完（提问会话，PROTOCOL 4.6）；cca 不支持时照旧走终端。
    var inApp = false
    /// 只有 cca 够新（`hello.forms`）才能在 App 里问完：旧版的这几条问的是终端里的那一套。
    var needsForms = false

    var id: String { arguments.joined(separator: "\u{1F}") }

    /// 半自动的终端：粘贴进去运行的那一行（cca 用绝对路径，不依赖那个终端的 PATH）。
    func pasteCommand(ccaPath: String) -> String {
        let run = ([ccaPath] + arguments).map(shellQuoteIfNeeded).joined(separator: " ")
        return directory.map { "cd \(shellQuoteIfNeeded($0)) && \(run)" } ?? run
    }

    init(arguments: [String], display: String? = nil, title: String, message: String = "", inApp: Bool = false, needsForms: Bool = false) {
        self.arguments = arguments
        self.display = display ?? displayCommand(arguments)
        self.title = title
        self.message = message
        self.inApp = inApp
        self.needsForms = needsForms
    }

    init(event: TerminalEvent) {
        self.init(arguments: event.argv, display: event.display, title: "需要在终端里完成", message: event.message)
    }

    static func login(prefix: [String], key: String, name: String) -> Handoff {
        Handoff(arguments: prefix + ["login", key], title: "登录 \(name)", message: "在终端里登录这个账号，完成后回到 cca。")
    }

    /// `cca login` 会先退出现在的登录，再登录一次：改过密码、登录失效、登错号时用。
    static func relogin(prefix: [String], key: String, name: String) -> Handoff {
        Handoff(arguments: prefix + ["login", key], title: "重新登录 \(name)",
                message: "会先退出这个账号现在的登录，再在它专属的浏览器窗口里重新登录，完成后回到 cca。")
    }

    /// 在终端里用这个账号（`cca @账号`），从 directory 启动。
    static func session(prefix: [String], key: String, name: String, directory: String) -> Handoff {
        var h = Handoff(arguments: prefix + ["@" + key], title: "在终端里打开 \(name)")
        h.directory = directory
        h.staysOpen = true
        h.systemTerminal = false
        return h
    }

    static func edit(prefix: [String]) -> Handoff {
        Handoff(arguments: prefix + ["edit"], title: "编辑 \(Texts.product(prefix.first.map(ToolID.init(rawValue:)) ?? .claude)) 账号表",
                message: "在终端里用编辑器修改账号表，保存后 cca 会检查格式。", inApp: true, needsForms: true)
    }

    static func addAccount(prefix: [String], product: String) -> Handoff {
        Handoff(arguments: prefix + ["add"], title: "添加 \(product) 账号", message: "在终端里按提示添加账号并登录。", inApp: true)
    }

    static func addAPI(prefix: [String] = []) -> Handoff {
        Handoff(arguments: prefix + ["add", "api"], title: "添加第三方 API 账号", message: "在终端里填写接口地址、模型和 key（key 存进钥匙串）。", inApp: true)
    }

    /// 更换第三方 API 账号的 key；官方账号是记一把 API key 备查（record：cca 不拿它启动）。
    static func apiKey(prefix: [String], key: String, name: String, record: Bool = false) -> Handoff {
        Handoff(arguments: prefix + ["key", key], title: record ? "记录 \(name) 的 API key" : "更换 \(name) 的 API key",
                message: "在终端里输入 key，存进钥匙串。", inApp: true)
    }

    static func remove(prefix: [String], key: String, name: String) -> Handoff {
        Handoff(arguments: prefix + ["rm", key], title: "删除 \(name)", message: "在终端里确认后删除这个账号。", inApp: true)
    }

    static let routerSetup = Handoff(arguments: ["router", "setup"], title: "配置路由器", message: "在终端里按提示配置路由器。", inApp: true, needsForms: true)
    static let localSetup = Handoff(arguments: ["local", "setup"], title: "配置本机 Clash", message: "在终端里按提示配置本机的 Clash。", inApp: true, needsForms: true)
    static let install = Handoff(arguments: ["install"], title: "安装 cca", message: "在终端里完成 cca 的安装。")

    /// 用官方安装脚本装 claude / codex / agy 命令（`cca [codex|agy] install-cli`，`hello.install_cli`）：先直连，不通时问过再经内置代理装。
    static func installCLI(prefix: [String], tool: ToolID) -> Handoff {
        Handoff(arguments: prefix + ["install-cli"], title: "安装 \(Texts.cliCommand(tool)) 命令",
                message: "在终端里用官方安装脚本安装；直连不通时会问要不要经内置代理装。装好后回到 cca。")
    }
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
    /// 结束时等按任意键再关窗口（账号的会话留在 shell 里，这个窗口接着能用）。文件一开始就挪走再删掉（bash 已经打开了它，
    /// 照样往下读；这样中途关窗口也不会留下文件）。挪走这一步和 App 的 `reclaim` 抢同一个 rename：App 收回了就不运行，
    /// 不会换了终端又跑一遍。dockTidy：cca 有 `_dock-tidy`（`hello.dock`）时，系统终端退出后去掉程序坞里它留下的图标。
    static func script(_ h: Handoff, ccaPath: String, dockTidy: Bool = false) -> String {
        let command = ([ccaPath] + h.arguments).map(shellQuote).joined(separator: " ")
        let enter = h.directory.map { dir in
            "cd -- \(shellQuote(dir)) || { echo \(shellQuote("打不开这个文件夹：" + dir)); cca_wait_close; exit 1; }\n"
        } ?? ""
        let finish = h.staysOpen ? """
        if [ "$status" -ne 0 ]; then echo "命令结束（退出码 ${status}）。"; fi
        exec "${SHELL:-/bin/zsh}" -l

        """ : """
        if [ "$status" -eq 0 ]; then echo "完成。回到 cca App 会自动刷新。"; else echo "命令结束（退出码 ${status}）。"; fi
        if [ -t 0 ]; then cca_wait_close; fi
        exit "$status"

        """
        return """
        #!/bin/bash
        # cca App 的终端交接：运行一次就删掉
        mv -- "$0" "$0.run" 2>/dev/null || exit 0
        rm -f -- "$0.run"
        export PATH="${PATH:-/usr/bin:/bin:/usr/sbin:/sbin}:$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin"
        unset CCA_TTY CCA_YES CCA_GRANT CCA_EVENTS CCA_RO CCA_SKIP_CHECK CCA_GUI CCA_PROMPT
        cca_tidy=\(dockTidy ? shellQuote(ccaPath) : "''")
        \(waitClose)
        \(enter)printf '%s\\n\\n' \(shellQuote("$ " + h.display))
        \(command)
        status=$?
        echo
        \(finish)
        """
    }

    /// 等按键，按了就关窗口。系统自带的“终端”进程结束后默认留着窗口（[进程已完成]，配置文件里的“当 shell 退出时”），要自己关：
    /// 趁脚本还在跑（busy）认出窗口的 id（已经结束的窗口还报着原来的 tty，新窗口会重用这个号，只按 tty 认可能认错）。
    /// 认出来就在后台（setsid：关窗口、退出“终端”都不影响它）等脚本结束：按了键、点了关闭、^C 都算，经一根管道（脚本退出时写端关上，
    /// 不轮询）。然后等 shell 退出、关这个窗口；“终端”没有别的窗口了（别的标签页、用户自己的窗口、最小化的都算）就退出它，免得它一直开在程序坞里；
    /// cca 够新时（`cca_tidy` 是 cca 的路径）再交给 `cca _dock-tidy`，去掉程序坞「最近使用的 App」里它留下的图标：它只是把活交给 cca 的
    /// 后台小 App 就退出（从这个窗口里起的进程都算“终端”的下属，还活着时重启的程序坞会把“终端”又加回去，所以这里什么都不能留下）。
    /// 窗口里还有别的标签页时不关（AppleScript 关不了单个标签页）。Terminal 发给自己的 Apple Event 不用授权（实测）；认不出、发不了就照旧留着窗口。
    /// “终端”已经退出了（用户按了 ⌘Q）就不再找它：`tell` 会把它重新打开。
    private static let waitClose = #"""
    cca_wait_close() {
    	local window="" p c tpid="" tapp="" as close
    	if [ "${TERM_PROGRAM:-}" = Apple_Terminal ]; then
    		window=$(perl -e 'alarm 3; exec @ARGV' osascript -e 'on run argv' -e 'tell application "Terminal"' \
    			-e 'repeat with w in windows' \
    			-e 'if (count of tabs of w) is 1 and tty of tab 1 of w is item 1 of argv and busy of tab 1 of w then return id of w' \
    			-e 'end repeat' -e 'end tell' -e 'end run' "$(tty)" 2>/dev/null)
    		p=$PPID
    		while [ "${p:-0}" -gt 1 ] 2>/dev/null; do
    			c=$(ps -o comm= -p "$p" 2>/dev/null)
    			case "$c" in */Contents/MacOS/Terminal) tpid=$p tapp=${c%/Contents/MacOS/Terminal}; break ;; esac
    			p=$(ps -o ppid= -p "$p" 2>/dev/null | tr -d ' ')
    		done
    	fi
    	if [ -n "$window" ]; then
    		as='on run argv
    	set i to (item 1 of argv) as integer
    	if application "Terminal" is not running then return ""
    	tell application "Terminal"
    		repeat 50 times
    			if not (exists window id i) then exit repeat
    			if not busy of tab 1 of window id i then
    				close window id i
    				exit repeat
    			end if
    			delay 0.1
    		end repeat
    		if (count of windows) > 0 then return ""
    		quit
    	end tell
    	return "quit"
    end run'
    		close='cat >/dev/null
    [ "$(osascript -e "$1" "$2" 2>/dev/null)" = quit ] && [ -n "$3" ] && [ -n "$5" ] && exec "$5" _dock-tidy "$4" "$3"'
    		exec 7> >(exec perl -MPOSIX -e 'POSIX::setsid(); exec @ARGV' /bin/bash -c "$close" cca-close "$as" "$window" "$tpid" "$tapp" "$cca_tidy" >/dev/null 2>&1)
    	fi
    	read -r -s -n 1 -p "按任意键关闭…" _; echo
    }
    """#

    /// 收回还没运行的交接脚本（选的终端没运行它）：true 表示收回了，它不会再被运行；false 表示已经开始运行了（或者已经没了）。
    static func reclaim(_ url: URL) -> Bool {
        let taken = url.appendingPathExtension("cancelled")
        guard rename(url.path, taken.path) == 0 else { return false }
        try? FileManager.default.removeItem(at: taken)
        return true
    }

    /// 写好 `.command` 文件（755），返回它的位置；打开它（用选好的终端）由 App 做。
    static func write(_ handoff: Handoff, ccaPath: String, directory: URL = defaultDirectory(), dockTidy: Bool = false) throws -> URL {
        let fm = FileManager.default
        try fm.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        cleanup(directory: directory)
        let url = directory.appendingPathComponent("cca-\(UUID().uuidString.prefix(8)).command")
        let text = script(handoff, ccaPath: ccaPath, dockTidy: dockTidy)
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
