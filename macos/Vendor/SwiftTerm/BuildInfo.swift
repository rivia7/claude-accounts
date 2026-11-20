// 这一份是 cca 写的，不是 SwiftTerm 的源码：上游用 SwiftPM 的构建插件现生成 `SwiftTermBuildInfo`，这里没有 SwiftPM，
// 给一份固定的。只有终端回答“你是什么版本”（XTVERSION）时用到。换 SwiftTerm 的版本时跟着改。
public enum SwiftTermBuildInfo {
    public static let branch: String? = nil
    public static let tag: String? = "v1.20.0"
    public static let commit: String? = "5d14406844143538cd8f8851d2d8a67c1fe443e5"
    public static let hasUncommittedChanges: Bool? = false
    public static let version: String = "1.20.0"
}
