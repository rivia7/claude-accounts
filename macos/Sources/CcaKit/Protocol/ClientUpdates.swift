import Foundation

/// `cca clients … --json`：检查和准备更新由后端负责；安装必须绑定本次检查的 token 并单独确认。
struct ClientUpdatesDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "client_updates"
    var version: Int?
    var kind: String?
    var generatedEpoch: Double?
    var mode: String?
    /// 下次该后台检查的时间：App 睡到这时候再来问（关了自动检查、没有客户端时是 nil）
    var nextCheckEpoch: Double?
    var clients: [ClientUpdateEntry]

    enum CodingKeys: String, CodingKey {
        case version, kind, mode, clients
        case generatedEpoch = "generated_epoch"
        case nextCheckEpoch = "next_check_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        nextCheckEpoch = c.double(.nextCheckEpoch)
        version = c.int(.version)
        kind = c.string(.kind)
        generatedEpoch = c.double(.generatedEpoch)
        mode = c.string(.mode)
        clients = c.lossyArray(ClientUpdateEntry.self, .clients) ?? []
    }

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

    enum CodingKeys: String, CodingKey {
        case id, label, kind, path, status, message
        case installedVersion = "installed_version", availableVersion = "available_version", stagedVersion = "staged_version"
        case checkedEpoch = "checked_epoch", installToken = "install_token", canInstall = "can_install"
        case packageVersion = "package_version", packageBuild = "package_build"
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
