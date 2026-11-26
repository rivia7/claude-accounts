import Foundation

// 远程启动（PROTOCOL 9.4）：模板的参数在 App 里是一行字（ShellWords 拆成一项一项、拼回一行），新建、修改模板的弹层（RemoteTemplateDraft）。

/// 照 shell 的写法把一行字拆成参数，或者反过来拼成一行：空白（空格、Tab、换行）隔开；单引号里的原样；双引号里 `\` 只让 `"`、`\`、
/// `$`、`` ` `` 原样（别的 `\` 留着）；引号外的 `\` 让下一个字符原样。不做变量、通配符、命令替换：`$HOME`、`*`、`~` 都原样交给 cca。
enum ShellWords {
    enum Problem: Error, Equatable {
        /// 单引号没有配对
        case openSingle
        /// 双引号没有配对
        case openDouble
        /// 最后一个字符是 `\`
        case trailingBackslash

        var message: String {
            switch self {
            case .openSingle: return "参数里的单引号没有配对"
            case .openDouble: return "参数里的双引号没有配对"
            case .trailingBackslash: return "参数最后多了一个 \\"
            }
        }
    }

    private static let blank: Set<Unicode.Scalar> = [" ", "\t", "\n", "\r"]
    /// 双引号里 `\` 后面跟着这几个时才算转义
    private static let escapable: Set<Unicode.Scalar> = ["\"", "\\", "$", "`"]

    static func split(_ line: String) -> Result<[String], Problem> {
        let s = Array(line.unicodeScalars)
        var words: [String] = []
        var word = String.UnicodeScalarView()
        var inWord = false   // 引号引起来的空字符串（''）也是一项
        var i = 0
        func finish() {
            if inWord { words.append(String(word)) }
            word = String.UnicodeScalarView()
            inWord = false
        }
        while i < s.count {
            let c = s[i]
            if blank.contains(c) {
                finish()
                i += 1
            } else if c == "'" {
                inWord = true
                guard let end = s[(i + 1)...].firstIndex(of: "'") else { return .failure(.openSingle) }
                word.append(contentsOf: s[(i + 1)..<end])
                i = end + 1
            } else if c == "\"" {
                inWord = true
                i += 1
                var closed = false
                while i < s.count {
                    let d = s[i]
                    if d == "\"" {
                        closed = true
                        i += 1
                        break
                    }
                    if d == "\\", i + 1 < s.count, escapable.contains(s[i + 1]) {
                        word.append(s[i + 1])
                        i += 2
                    } else if d == "\\", i + 1 < s.count, s[i + 1] == "\n" {   // 续行
                        i += 2
                    } else {
                        word.append(d)
                        i += 1
                    }
                }
                if !closed { return .failure(.openDouble) }
            } else if c == "\\" {
                guard i + 1 < s.count else { return .failure(.trailingBackslash) }
                if s[i + 1] != "\n" {   // 反斜杠加换行是续行：两个都不要
                    word.append(s[i + 1])
                    inWord = true
                }
                i += 2
            } else {
                word.append(c)
                inWord = true
                i += 1
            }
        }
        finish()
        return .success(words)
    }

    /// 拼成一行给人看、给人改：要引的才引（单引号），空格隔开。拆回去是原样
    static func join(_ args: [String]) -> String {
        args.map(shellQuoteIfNeeded).joined(separator: " ")
    }
}

/// 新建、修改一个模板的弹层里填的东西（这套工具链里 @State 用不了：放在被观察的对象里）。存的时候交给 cca 校验，
/// 不行的原因（cca 说的，或者参数拆不开）写在弹层里，接着改。
@MainActor @Observable
final class RemoteTemplateDraft: Identifiable {
    nonisolated let id = UUID()
    /// 改的是哪一个（新建时是 nil）：存的时候带 `--replace <编号>`
    let original: RemoteTemplate?
    var name: String
    var tool: ToolID
    /// 账号表的键（账号页里的那个）；还没选是空的
    var account: String
    /// 文件夹（绝对路径）；还没选是空的
    var dir: String
    /// 参数：一行字，照 shell 的写法（ShellWords）
    var args: String
    /// 没存成的原因（写在弹层里）
    var error: String?
    /// 正在存（按钮先停用）
    var saving = false

    init(original: RemoteTemplate?, name: String, tool: ToolID, account: String, dir: String, args: String) {
        self.original = original
        self.name = name
        self.tool = tool
        self.account = account
        self.dir = dir
        self.args = args
    }

    /// 名字、账号、文件夹都有了才能存（别的交给 cca 校验）
    var canSave: Bool {
        !saving && !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !account.isEmpty && !dir.isEmpty
    }
}

extension RemoteTemplateDraft: Equatable {
    nonisolated static func == (a: RemoteTemplateDraft, b: RemoteTemplateDraft) -> Bool { a === b }
}
