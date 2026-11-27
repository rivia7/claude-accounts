import Foundation

/// `cca _gui hello`（PROTOCOL 2.1）。
struct HelloDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "hello"

    var version: Int?
    var kind: String?
    var api: Int?
    /// 提问会话的版本（PROTOCOL 4.6）；旧版 cca 没有这个字段，流程都走终端。
    var prompt: Int?
    /// 有 `cca reorder`（App 调整账号顺序时写回账号表）；旧版 cca 没有这个字段
    var reorder: Int?
    /// 配置路由器、配置本机 Clash、账号表能在 App 里填表（`PromptAPI.forms`）；旧版 cca 没有这个字段，这几条走终端
    var forms: Int?
    /// 有 `cca _dock-tidy`（PROTOCOL 6）：终端交接退出系统终端后，去掉程序坞「最近使用的 App」里它留下的图标；旧版 cca 没有这个字段
    var dock: Int?
    /// 有 `cca [codex|agy] install-cli`（辅助安装：用官方安装脚本装 claude / codex / agy 命令，直连不通时问过再经内置代理装）；
    /// 旧版 cca 没有这个字段，“没装 X 命令”的地方不给「安装」
    var installCLI: Int?
    /// 有折合金额（`_gui state` 的 `cost`、`cost_total`，`cca cost sync --json`，PROTOCOL 2.2、3.5）；旧版 cca 没有这个字段，
    /// App 不显示折合金额、不调 `cost sync`（版本不是 CostAPI.version 时也一样）
    var cost: Int?
    /// 有邮箱助手的 App 入口（账号页的「邮箱」一节，PROTOCOL 2.4、4.11：`mail setup`、`_gui mail-list`、`mail open --msg`、`mail forget`）；
    /// 旧版 cca 没有这个字段，App 不显示「邮箱」一节、不调这些命令
    var mail: Int?
    /// 有远程会话的 App 入口（「远程会话」一页，PROTOCOL 9：`remote list --json`、配对、`remote attach`）；旧版 cca 没有这个字段，
    /// App 不显示这一页、不调这些命令（旧版 cca 会把 `remote` 当成给 claude 的参数）
    var remote: Int?
    /// 有 `cca remote end`（结束这台 Mac 上的一个会话，PROTOCOL 9.2）；旧版 cca 没有这个字段，App 不给「结束」
    var remoteEnd: Int?
    /// 有 `cca remote pair --web`（和手机配对：手机在浏览器里接入，PROTOCOL 9.2）；旧版 cca 没有这个字段，App 不给「和手机配对…」
    var remoteWeb: Int?
    /// 有远程启动（手机用这台 Mac 上的模板新建会话：`cca remote template`、`cca remote allow <配对的 id> launch on|off`，PROTOCOL 9.4）；
    /// 旧版 cca 没有这个字段，App 不显示模板、不给「允许远程启动」
    var remoteLaunch: Int?
    /// 远程启动不只给手机（2026-11-27 起）：配对过的电脑也能打开「允许远程启动」，这台 Mac 也能用配对过的另一台机器上的模板在那台机器上
    /// 新建会话（`remote list --json` 的 `peer_launchers`、`cca remote launch … --json`，PROTOCOL 9.5）。旧版 cca 没有这个字段：
    /// 只有手机能打开，App 不给「新建会话…」
    var remoteLaunchMachines: Int?
    /// 有 Team（`cca team …`，PROTOCOL 4.14；state 的 `config.teams`、账号的 `team`）；旧版 cca 没有这个字段，App 不显示 Team 的那一块、
    /// 不按 Team 筛选、不调 `cca team`（版本不是 TeamAPI.version 时也一样）
    var team: Int?
    /// 有导出、导入、修改 Team 的配置（`cca team export|import|edit`，PROTOCOL 4.14）；旧版 cca 没有这个字段，App 不给「导出配置…」
    /// 「导入配置…」「在终端里修改配置…」（旧版 cca 的 `team import` 只认 `--mine`）
    var teamConfig: Int?
    var ccaVersion: String?
    var devMode: Bool
    /// 缺这个字段时当作已安装（不无故显示安装引导）。
    var installed: Bool
    var selfPath: String?
    var home: String?
    var tools: [ToolID: HelloTool]
    var gui: HelloGUI?

    enum CodingKeys: String, CodingKey {
        case version, kind, api, prompt, reorder, forms, dock, cost, mail, remote, team
        case installCLI = "install_cli"
        case remoteEnd = "remote_end"
        case remoteWeb = "remote_web"
        case remoteLaunch = "remote_launch"
        case remoteLaunchMachines = "remote_launch_machines"
        case teamConfig = "team_config"
        case ccaVersion = "cca_version"
        case devMode = "dev_mode"
        case installed
        case selfPath = "self"
        case home, tools, gui
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        api = c.int(.api)
        prompt = c.int(.prompt)
        reorder = c.int(.reorder)
        forms = c.int(.forms)
        dock = c.int(.dock)
        installCLI = c.int(.installCLI)
        cost = c.int(.cost)
        mail = c.int(.mail)
        remote = c.int(.remote)
        remoteEnd = c.int(.remoteEnd)
        remoteWeb = c.int(.remoteWeb)
        remoteLaunch = c.int(.remoteLaunch)
        remoteLaunchMachines = c.int(.remoteLaunchMachines)
        team = c.int(.team)
        teamConfig = c.int(.teamConfig)
        ccaVersion = c.string(.ccaVersion)
        devMode = c.bool(.devMode) ?? false
        installed = c.bool(.installed) ?? true
        selfPath = c.string(.selfPath)
        home = c.string(.home)
        var t: [ToolID: HelloTool] = [:]
        for (k, v) in c.lossyDictionary(HelloTool.self, .tools) ?? [:] {
            t[ToolID(rawValue: k)] = v
        }
        tools = t
        gui = c.lossy(HelloGUI.self, .gui)
    }

    /// `cca codex …`、`cca agy …` 只有这里为真时才能调用（PROTOCOL 1）：Claude 一直能用，别的缺字段一律当作没启用。
    func enabled(_ t: ToolID) -> Bool { t == .claude || (tools[t]?.enabled ?? false) }
    /// 启用了的工具（认识的），按 Claude、Codex、Antigravity 的顺序。
    var enabledTools: [ToolID] { ToolID.allKnown.filter { enabled($0) } }
    var codexEnabled: Bool { enabled(.codex) }
}

struct HelloTool: Decodable, Sendable, Equatable {
    var enabled: Bool
    var bin: String?

    enum CodingKeys: String, CodingKey { case enabled, bin }

    init(enabled: Bool, bin: String?) {
        self.enabled = enabled
        self.bin = bin
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        enabled = c.bool(.enabled) ?? false
        bin = c.string(.bin)
    }
}

struct HelloGUI: Decodable, Sendable, Equatable {
    var app: String?
    var appVersion: String?
    var stagedVersion: String?

    enum CodingKeys: String, CodingKey {
        case app
        case appVersion = "app_version"
        case stagedVersion = "staged_version"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        app = c.string(.app)
        appVersion = c.string(.appVersion)
        stagedVersion = c.string(.stagedVersion)
    }
}
