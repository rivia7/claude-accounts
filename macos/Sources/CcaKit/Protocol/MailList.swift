import Foundation

/// 账号页「邮箱」一节按需拉取的登录邮件列表（`cca _gui mail-list --account KEY [小时]`，PROTOCOL 2.4）：
/// 实时连 Gmail、只读 IMAP，列出最近登录相关的邮件。**绝不含登录链接本身**，只有链接的“样子”（`linkShape`）。
/// 要 `hello.mail`、账号的 `can.mail`。
struct MailListDoc: CcaDocument, Equatable {
    static let expectedKind: String? = "mail-list"

    var version: Int?
    var kind: String?
    var generatedAt: Double?
    /// 这次列的是哪个账号（名字，cca 给的），不写账号时是 nil
    var account: String?
    /// 账号表里的键（打开某封时要回传给 `cca mail open KEY --msg …`）
    var key: String?
    /// 这次连的是哪个 Gmail（没配置时可能是 nil）
    var user: String?
    /// 看了哪些邮件夹（给人看的名字）
    var folders: [String]
    /// 还有更多（超过 50 封没列全）
    var truncated: Bool
    var rows: [MailRow]
    /// 连不上 / 没配置时的原因（成功时是 nil）
    var error: MailListError?

    enum CodingKeys: String, CodingKey {
        case version, kind, account, key, user, folders, truncated, rows, error
        case generatedAt = "generated_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = c.int(.version)
        kind = c.string(.kind)
        generatedAt = c.double(.generatedAt)
        account = c.string(.account)
        key = c.string(.key)
        user = c.string(.user)
        folders = c.lossyArray(String.self, .folders) ?? []
        truncated = c.bool(.truncated) ?? false
        rows = c.lossyArray(MailRow.self, .rows) ?? []
        error = c.lossy(MailListError.self, .error)
    }
}

/// 连不上 / 没配置的原因。`code`：`not_configured` | `no_password` | `route` | `connect`（都当字符串收）。
struct MailListError: Decodable, Sendable, Equatable {
    var code: String?
    var message: String?

    enum CodingKeys: String, CodingKey { case code, message }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        code = c.string(.code)
        message = c.string(.message)
    }

    var text: String { nonEmpty(message) ?? "连不上邮箱" }
}

/// 一封登录相关的邮件（按收到时间倒序）。标题、发件人、收件人是邮件头里的（发件人写的，照实显示，不参与判断）。
struct MailRow: Decodable, Sendable, Equatable, Identifiable {
    var gotEpoch: Double?
    /// 给人看的邮件夹名（所有邮件 / 垃圾邮件 / 收件箱）
    var folder: String?
    var subject: String?
    var from: [String]
    var to: [String]
    /// 按收件人认出的账号（认不出为 nil）
    var account: MailRowAccount?
    /// Gmail 写的验证结果（不是 Gmail 写的为 nil）
    var dkim: [MailDKIM]?
    var verdict: MailVerdict
    /// 登录链接的“样子”（协议、主机、路径，后面只说多长，**不含链接内容**）；只有通过核对时才有
    var linkShape: String?
    /// 能不能在这个账号的浏览器里打开（通过核对且认出了账号）
    var openable: Bool
    /// 打开时要回传的不透明句柄（只有 openable 时才有）
    var msg: MailHandle?

    /// 稳定 id：句柄（有的话）或 邮件夹+时间
    var id: String { msg.map { "\($0.folder):\($0.uid):\($0.uidvalidity)" } ?? "\(folder ?? ""):\(gotEpoch ?? 0)" }

    enum CodingKeys: String, CodingKey {
        case folder, subject, from, to, account, dkim, verdict, openable, msg
        case gotEpoch = "got_epoch"
        case linkShape = "link_shape"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        gotEpoch = c.double(.gotEpoch)
        folder = c.string(.folder)
        subject = c.string(.subject)
        from = c.lossyArray(String.self, .from) ?? []
        to = c.lossyArray(String.self, .to) ?? []
        account = c.lossy(MailRowAccount.self, .account)
        dkim = c.lossyArray(MailDKIM.self, .dkim)
        verdict = c.lossy(MailVerdict.self, .verdict) ?? MailVerdict(ok: false, why: nil, whyText: nil)
        linkShape = c.string(.linkShape)
        openable = c.bool(.openable) ?? false
        msg = c.lossy(MailHandle.self, .msg)
    }

    /// 没有标题时的占位
    var subjectText: String { nonEmpty(subject) ?? "（没有标题）" }
}

struct MailRowAccount: Decodable, Sendable, Equatable {
    var key: String?
    var name: String?

    enum CodingKeys: String, CodingKey { case key, name }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        key = c.string(.key)
        name = c.string(.name)
    }
}

struct MailDKIM: Decodable, Sendable, Equatable {
    var result: String?
    var domain: String?

    enum CodingKeys: String, CodingKey { case result, domain }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        result = c.string(.result)
        domain = c.string(.domain)
    }
}

/// cca 的核对结论。`ok` 为真表示是这个账号的真登录邮件；`why` / `whyText` 是没通过的原因（字符串，cca 给的，App 不重算）。
struct MailVerdict: Decodable, Sendable, Equatable {
    var ok: Bool
    var why: String?
    var whyText: String?

    init(ok: Bool, why: String?, whyText: String?) {
        self.ok = ok
        self.why = why
        self.whyText = whyText
    }

    enum CodingKeys: String, CodingKey {
        case ok, why
        case whyText = "why_text"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        ok = c.bool(.ok) ?? false
        why = c.string(.why)
        whyText = c.string(.whyText)
    }
}

/// 打开某封登录邮件时回传给 cca 的不透明句柄（App 只当字符串用，不解析）。
struct MailHandle: Decodable, Sendable, Equatable {
    var folder: String
    var uid: Int
    var uidvalidity: Int

    enum CodingKeys: String, CodingKey { case folder, uid, uidvalidity }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard let folder = c.string(.folder), let uid = c.int(.uid) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "mail handle without folder/uid"))
        }
        self.folder = folder
        self.uid = uid
        uidvalidity = c.int(.uidvalidity) ?? 0
    }

    /// `cca mail open KEY --msg` 要的 `文件夹:uid:uidvalidity`
    var argument: String { "\(folder):\(uid):\(uidvalidity)" }
}

/// App 里记着的一个账号的登录邮件列表（只在内存里）：最近一次拉到的，和最近一次没拉成的硬错误。
struct MailListSnapshot: Equatable, Sendable {
    var doc: MailListDoc?
    /// 连不上 cca（崩溃、超时）的硬错误；`not_configured` 这些软错误在 `doc.error` 里。拉成了是 nil
    var error: String?
    /// 最近一次拉到的时间
    var fetchedEpoch: Double?
}
