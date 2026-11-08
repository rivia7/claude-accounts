import Foundation

/// `cca clients … --json`：检查和准备更新由后端负责；安装必须绑定本次检查的 token 并单独确认。
struct ClientUpdatesDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "client_updates"
    var version: Int?
    var kind: String?
    var generatedEpoch: Double?
    var mode: String?
    var clients: [ClientUpdateEntry]

    enum CodingKeys: String, CodingKey {
        case version, kind, mode, clients
        case generatedEpoch = "generated_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        generatedEpoch = c.double(.generatedEpoch)
        mode = c.string(.mode)
        clients = c.lossyArray(ClientUpdateEntry.self, .clients) ?? []
    }
}

struct ClientUpdateEntry: Decodable, Sendable, Equatable, Identifiable {
    var id: String
    var label: String?
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
        case id, label, path, status, message
        case installedVersion = "installed_version", availableVersion = "available_version", stagedVersion = "staged_version"
        case checkedEpoch = "checked_epoch", installToken = "install_token", canInstall = "can_install"
        case packageVersion = "package_version", packageBuild = "package_build"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = c.string(.id) ?? ""
        label = c.string(.label)
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

    /// 是否可安装只采用后端给的结论；已失效的检查即使带旧 token 也不能复用。
    var readyToInstall: Bool {
        canInstall && nonEmpty(installToken) != nil && status != "needs_recheck"
    }

    var statusText: String {
        switch status {
        case "missing": return "尚未安装"
        case "current": return "已是最新版本"
        case "checking": return "正在检查"
        case "busy": return "更新正在进行"
        case "unsupported": return "当前系统暂无适配包"
        case "staged": return "更新已准备好"
        case "running": return "客户端正在运行"
        case "blocked": return "暂不能安装"
        case "error": return "更新检查失败"
        case "updated": return "已更新"
        case "unverified": return "等待兼容性验证"
        case "needs_recheck": return "需要重新检查"
        default: return "等待检查结果"
        }
    }

    var detail: String {
        if status == "needs_recheck" { return "目标版本或软件包校验值已变化，请重新检查后再安装。" }
        return nonEmpty(message) ?? statusText
    }
}
