// swift-tools-version:5.9
// iOS App 的协议核心（没有界面）：配对、列会话、接入。App 的 Xcode 工程把它当本地包引用；
// 没有 Xcode 时 tests/ios_kit.rs 直接用 swiftc 编它的源码测（见 docs/dev/ios.md）
import PackageDescription

let package = Package(
    name: "CcaRemoteKit",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [.library(name: "CcaRemoteKit", targets: ["CcaRemoteKit"])],
    targets: [.target(name: "CcaRemoteKit", path: "Sources/CcaRemoteKit")]
)
