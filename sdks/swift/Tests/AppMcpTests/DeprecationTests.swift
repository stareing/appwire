@testable import AppMcp
import XCTest

/// 工具弃用声明（spec/protocol.md 3.7）：缺省、透传、更新替换 / 清除、补丁型 update 与启停不丢、非法拒绝。不需要 Host。
final class DeprecationTests: XCTestCase {
    private let full = Deprecation(message: "改用 d.new", replacement: "d.new", until: "2027-06-30")
    private let onlyMessage = Deprecation(message: "即将移除")

    private func makeClient() throws -> AppMcpClient {
        try AppMcpClient(config: AppMcpConfig(appId: "swift-deprecated", appName: "弃用声明", hostURL: "ws://127.0.0.1:9"))
    }

    func testDeprecationReachesSpec() throws {
        let client = try makeClient()
        defer { client.stop() }
        XCTAssertNil(try client.tool("d.plain", description: "缺省") { (_: NoArguments, _) in 1 }.specForTest.deprecated)
        let f = try client.tool("d.full", description: "完整", deprecated: full) { (_: NoArguments, _) in 1 }
        XCTAssertEqual(f.specForTest.deprecated, full)
        let bg = try client.backgroundTool("d.bg", description: "后台", deprecated: onlyMessage) { (_: NoArguments, _) in 1 }
        XCTAssertEqual(bg.specForTest.deprecated, onlyMessage)
        XCTAssertNil(onlyMessage.replacement)
        XCTAssertNil(onlyMessage.until)
    }

    func testUpdateKeepsReplacesAndClears() throws {
        let client = try makeClient()
        defer { client.stop() }
        let h = try client.tool("d.t", description: "旧", deprecated: full) { (_: NoArguments, _) in 1 }
        try h.update(description: "新描述")
        try h.setEnabled(false)
        try h.setEnabled(true)
        XCTAssertEqual(h.specForTest.deprecated, full, "补丁型 update / setEnabled 不得丢失 deprecated")
        let withFull = client.toolsHash
        try h.update(deprecated: onlyMessage)
        XCTAssertEqual(h.specForTest.deprecated, onlyMessage)
        let replaced = client.toolsHash
        XCTAssertNotEqual(withFull, replaced, "替换后的声明同步到原生层（进 toolsHash）")
        try h.update { $0.deprecated = nil }
        XCTAssertNil(h.specForTest.deprecated)
        XCTAssertNotEqual(client.toolsHash, replaced, "清除后的声明同步到原生层")
        XCTAssertNotEqual(client.toolsHash, withFull)
        try h.update { $0.deprecated = self.full }
        XCTAssertEqual(client.toolsHash, withFull)
    }

    func testInvalidDeprecationRejected() throws {
        let client = try makeClient()
        defer { client.stop() }
        func expectInvalidConfig(_ what: String, _ body: () throws -> Void) {
            XCTAssertThrowsError(try body(), what) { err in
                guard case AppMcpError.InvalidConfig = err else { return XCTFail("\(what)：期望 InvalidConfig，得到 \(err)") }
            }
        }
        let bad = [
            Deprecation(message: ""),
            Deprecation(message: "   "),
            Deprecation(message: String(repeating: "x", count: 501)),
            Deprecation(message: "m", replacement: "bad name!"),
            Deprecation(message: "m", replacement: "d.bad"),
            Deprecation(message: "m", until: "2027-02-30"),
            Deprecation(message: "m", until: "2027/06/30"),
        ]
        for d in bad {
            expectInvalidConfig("\(d)") { try client.tool("d.bad", description: "非法", deprecated: d) { (_: NoArguments, _) in 1 } }
        }
        let ok = Deprecation(message: String(repeating: "x", count: 500), replacement: "d.missing", until: "2028-02-29")
        let h = try client.tool("d.ok", description: "合法", deprecated: ok) { (_: NoArguments, _) in 1 }
        expectInvalidConfig("指向自身") { try h.update(deprecated: Deprecation(message: "m", replacement: "d.ok")) }
        XCTAssertEqual(h.specForTest.deprecated, ok, "更新失败时保留原声明")
    }
}
