import Foundation

// MARK: - 运行

print("cca App 契约测试：\(fullMode ? "完整（手写数据 + 单元测试）" : "通用（只解码 + 不变量）")，目录 \(fixtureDir.path)")
_ = runGeneric()
if fullMode {
    fixtureAssertions()
    await rerunProtocolTests()
    await gateTests()
    textTests()
    environmentTests()
    locatorTests()
    eventTailTests()
    await shellTests()
    await handoffTests()
    await processTests()
    await processTransportTests()
    await watcherTests()
    await modelTests()
}
try? FileManager.default.removeItem(at: tmpRoot)
print("\n通过 \(H.passed)，失败 \(H.failed)")
exit(H.failed == 0 ? 0 : 1)
