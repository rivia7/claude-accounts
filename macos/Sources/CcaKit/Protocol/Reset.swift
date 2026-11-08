import Foundation

/// 重置额度（`cca codex reset KEY --json`，PROTOCOL 4.5）：用掉一次重置机会后的结果和重置后的用量。
struct ResetDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "reset"

    var version: Int?
    var kind: String?
    var tool: ToolID?
    var generatedEpoch: Double?
    var key: String?
    var outcome: ResetOutcome?
    /// 给人看的一句话（cca 写好的）
    var message: String?
    /// 重置后的用量（读不到为 nil：App 自己再刷新一次）
    var usage: UsageEntry?

    enum CodingKeys: String, CodingKey {
        case version, kind, tool
        case generatedEpoch = "generated_epoch"
        case key, outcome, message, usage
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        tool = c.lossy(ToolID.self, .tool)
        generatedEpoch = c.double(.generatedEpoch)
        key = c.string(.key)
        outcome = c.lossy(ResetOutcome.self, .outcome)
        message = c.string(.message)
        usage = c.lossy(UsageEntry.self, .usage)
    }

    /// 重置成了（也包括这一次早就重置过了）：用量应该已经清零
    var didReset: Bool { outcome == .reset || outcome == .alreadyRedeemed }
}
