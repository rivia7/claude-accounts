import Foundation

/// `cca clients … --json`：检查和准备更新由后端负责；安装必须绑定本次检查的 token 并单独确认。
struct ClientUpdatesDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "client_updates"
    var version: Int?
    var kind: String?
    var generatedEpoch: Double?
    var mode: String?
    /// 允许自动更新（「客户端更新」页的开关）：后台检查查到新版本就装，不再先问；只在自动检查开着时是 true。
    /// 旧版 cca 没有这个字段（nil）：没有这个开关
    var autoInstall: Bool?
    /// 下次该后台检查的时间：App 睡到这时候再来问（关了自动检查、没有客户端时是 nil）
    var nextCheckEpoch: Double?
    var clients: [ClientUpdateEntry]

    enum CodingKeys: String, CodingKey {
        case version, kind, mode, clients
        case generatedEpoch = "generated_epoch"
        case autoInstall = "auto_install"
        case nextCheckEpoch = "next_check_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        nextCheckEpoch = c.double(.nextCheckEpoch)
        version = c.int(.version)
        kind = c.string(.kind)
        generatedEpoch = c.double(.generatedEpoch)
        mode = c.string(.mode)
        autoInstall = c.bool(.autoInstall)
        clients = c.lossyArray(ClientUpdateEntry.self, .clients) ?? []
    }

    /// 有下载好、等桌面客户端退出后自动安装的：客户端退出时 App 马上检查一次（cca 会装上）
    var autoInstallPending: Bool { clients.contains(where: \.autoInstallPending) }

    /// 有新版本的客户端有几个（侧栏「客户端更新」右边的标签）
    var updatableCount: Int { clients.filter(\.hasUpdate).count }
}

struct ClientUpdateEntry: Decodable, Sendable, Equatable, Identifiable {
    var id: String
    var label: String?
    /// `desktop` | `cli`（终端里的 Claude Code、Codex）；旧版 cca 没有这个字段，都是桌面客户端
    var kind: String?
    var path: String?
    var installedVersion: String?
    var availableVersion: String?
    var stagedVersion: String?
    var status: String?
    var message: String?
    var checkedEpoch: Double?
    var installToken: String?
    var canInstall: Bool
    var packageVersion: String?
    var packageBuild: String?
    /// 允许自动更新、下载好了而客户端还开着：等它退出后由 cca 自动安装（安装按钮照样能用）
    var autoInstallPending: Bool

    enum CodingKeys: String, CodingKey {
        case id, label, kind, path, status, message
        case installedVersion = "installed_version", availableVersion = "available_version", stagedVersion = "staged_version"
        case checkedEpoch = "checked_epoch", installToken = "install_token", canInstall = "can_install"
        case packageVersion = "package_version", packageBuild = "package_build"
        case autoInstallPending = "auto_install_pending"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = c.string(.id) ?? ""
        label = c.string(.label)
        kind = c.string(.kind)
        path = c.string(.path)
        installedVersion = c.string(.installedVersion)
        availableVersion = c.string(.availableVersion)
        stagedVersion = c.string(.stagedVersion)
        status = c.string(.status)
        message = c.string(.message)
        checkedEpoch = c.double(.checkedEpoch)
        installToken = c.string(.installToken)
        canInstall = c.bool(.canInstall) ?? false
        packageVersion = c.string(.packageVersion)
        packageBuild = c.stringish(.packageBuild)
        autoInstallPending = c.bool(.autoInstallPending) ?? false
    }

    var isCLI: Bool { kind == "cli" }

    /// 有新版本：查到了（available）、下载好了（staged），或者下载好了、客户端还开着（running）。
    /// 出错、被拦住、要重新检查的不算（那些在卡片上说原因）。
    var hasUpdate: Bool { ["staged", "running", "available"].contains(status ?? "") }

    /// 是否可安装只采用后端给的结论；已失效的检查即使带旧 token 也不能复用。
    var readyToInstall: Bool {
        canInstall && nonEmpty(installToken) != nil && status != "needs_recheck"
    }

    var statusText: String {
        if autoInstallPending { return "退出后自动安装" }
        switch status {
        case "missing": return "还没装"
        case "current": return "已是最新版本"
        case "checking": return "正在检查"
        case "busy": return "正在更新"
        case "unsupported": return "这个系统暂时没有可用的安装包"
        case "staged": return "可以安装了"
        case "available": return "有新版本"
        case "running": return "请先退出客户端"
        case "blocked": return "现在不能安装"
        case "error": return "检查更新失败"
        case "updated": return "已更新"
        case "unverified": return "还没通过兼容性检查"
        case "needs_recheck": return "需要重新检查"
        default: return "等待检查结果"
        }
    }

    var detail: String {
        if status == "needs_recheck" { return "要安装的版本变了，请重新检查后再安装。" }
        return nonEmpty(message) ?? statusText
    }
}
