import Foundation

/// 检查更新（`cca update --check --json`，PROTOCOL 5.1）：GitHub 上这个版本跟着的最新版本、有没有比现在新、更新说明。
/// 开发模式看 hello 的 `dev_mode`（这份文档里没有）。
struct UpdateDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "update"

    var version: Int?
    var kind: String?
    var generatedEpoch: Double?
    var ccaVersion: String?
    var channel: UpdateChannel?
    var source: UpdateSource?
    var latest: String?
    var available: Bool
    /// 更新说明（GitHub Release 的说明，原样）
    var notes: String?
    /// 发布页
    var url: String?
    var publishedEpoch: Double?

    enum CodingKeys: String, CodingKey {
        case version, kind
        case generatedEpoch = "generated_epoch"
        case ccaVersion = "cca_version"
        case channel, source, latest, available, notes, url
        case publishedEpoch = "published_epoch"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        generatedEpoch = c.double(.generatedEpoch)
        ccaVersion = c.string(.ccaVersion)
        channel = c.lossy(UpdateChannel.self, .channel)
        source = c.lossy(UpdateSource.self, .source)
        latest = c.string(.latest)
        available = c.bool(.available) ?? false
        notes = c.string(.notes)
        url = c.string(.url)
        publishedEpoch = c.double(.publishedEpoch)
    }
}
