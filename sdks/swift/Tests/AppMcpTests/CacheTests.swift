@testable import AppMcp
import XCTest

/// 结果缓存声明（spec/protocol.md 3.6）：缺省、透传、更新替换 / 清除、补丁型 update 与启停不丢、越界拒绝。不需要 Host。
final class CacheTests: XCTestCase {
    private let private5s = CachePolicy(ttlMs: 5_000, scope: nil)
    private let shared1s = CachePolicy(ttlMs: 1_000, scope: .shared)

    private func makeClient() throws -> AppMcpClient {
        try AppMcpClient(config: AppMcpConfig(appId: "swift-cache", appName: "缓存声明", hostURL: "ws://127.0.0.1:9"))
    }

    func testCacheReachesSpec() throws {
        let client = try makeClient()
        defer { client.stop() }
        XCTAssertNil(try client.tool("c.plain", description: "缺省", risk: .read) { (_: NoArguments, _) in 1 }.specForTest.cache)
        let p = try client.tool("c.p", description: "私有", risk: .read, cache: private5s) { (_: NoArguments, _) in 1 }
        XCTAssertEqual(p.specForTest.cache, private5s)
        let bg = try client.backgroundTool("c.bg", description: "后台", risk: .read, cache: shared1s) { (_: NoArguments, _) in 1 }
        XCTAssertEqual(bg.specForTest.cache, shared1s)
        try client.resource("r.cached", description: "资源", cache: shared1s) { 1 }
    }

    func testUpdateKeepsReplacesAndClears() throws {
        let client = try makeClient()
        defer { client.stop() }
        let h = try client.tool("c.t", description: "读", risk: .read, cache: private5s) { (_: NoArguments, _) in 1 }
        try h.update(description: "新描述")
        try h.setEnabled(false)
        try h.setEnabled(true)
        XCTAssertEqual(h.specForTest.cache, private5s, "补丁型 update / setEnabled 不得丢失 cache")
        let withCache = client.toolsHash
        try h.update(cache: shared1s)
        XCTAssertEqual(h.specForTest.cache, shared1s)
        let replaced = client.toolsHash
        XCTAssertNotEqual(withCache, replaced, "替换后的声明同步到原生层（进 toolsHash）")
        try h.update { $0.cache = nil }
        XCTAssertNil(h.specForTest.cache)
        XCTAssertNotEqual(client.toolsHash, replaced, "清除后的声明同步到原生层")
        XCTAssertNotEqual(client.toolsHash, withCache)
        try h.update { $0.cache = self.private5s }
        XCTAssertEqual(client.toolsHash, withCache)
    }

    func testOutOfRangeTtlRejected() throws {
        let client = try makeClient()
        defer { client.stop() }
        func expectInvalidConfig(_ body: () throws -> Void) {
            XCTAssertThrowsError(try body()) { err in
                guard case AppMcpError.InvalidConfig = err else { return XCTFail("期望 InvalidConfig，得到 \(err)") }
            }
        }
        for ttl: UInt64 in [0, 86_400_001] {
            expectInvalidConfig { try client.tool("c.bad", description: "越界", risk: .read, cache: CachePolicy(ttlMs: ttl, scope: nil)) { (_: NoArguments, _) in 1 } }
            expectInvalidConfig { try client.resource("r.bad", description: "越界", cache: CachePolicy(ttlMs: ttl, scope: nil)) { 1 } }
        }
        let max = CachePolicy(ttlMs: 86_400_000, scope: nil)
        let h = try client.tool("c.ok", description: "上限", risk: .read, cache: max) { (_: NoArguments, _) in 1 }
        expectInvalidConfig { try h.update(cache: CachePolicy(ttlMs: 0, scope: nil)) }
        XCTAssertEqual(h.specForTest.cache, max, "更新失败时保留原声明")
    }
}
