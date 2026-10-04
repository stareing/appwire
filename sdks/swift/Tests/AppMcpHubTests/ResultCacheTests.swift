import AppMcp
import AppMcpHub
import Foundation
import XCTest

private struct CacheWaitTimeout: Error {}

/// 线程安全计数（App handler 在协作线程池上执行）。
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var n = 0
    func next() -> Int { lock.lock(); defer { lock.unlock() }; n += 1; return n }
    var value: Int { lock.lock(); defer { lock.unlock() }; return n }
}

/// 只读结果缓存（第 16 项 O3，spec/hub-api.md 3.20）：真实 App 声明 `cache` 的只读工具与资源 → 第二次命中（`cachedAgeMs`、
/// App 只执行一次）、`cacheBypass` 照常执行、`resultCache.maxEntries = 0` 关闭、`status().cache` 计数。
final class ResultCacheTests: XCTestCase {
    private func startKv(_ hub: Hub, gets: Counter, reads: Counter) async throws -> AppMcpClient {
        let app = try AppMcpClient(config: AppMcpConfig(appId: "kv", appName: "KV", hostURL: "ws://\(hub.listenAddr ?? "")/app"))
        try app.backgroundTool("get", description: "读取", risk: .read, cache: CachePolicy(ttlMs: 60_000, scope: nil)) {
            (_: NoArguments, _) in ["n": gets.next()]
        }
        try app.resource("snapshot", description: "快照", cache: CachePolicy(ttlMs: 60_000, scope: .shared)) { ["v": reads.next()] }
        app.start()
        let deadline = Date().addingTimeInterval(10)
        while !hub.tools(ToolFilter(apps: ["kv"])).contains(where: { $0.name == "kv.get" }) {
            guard Date() < deadline else { throw CacheWaitTimeout() }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        return app
    }

    func testCacheHitBypassAndStatus() async throws {
        let hub = try Hub(config: HubConfig(listen: "127.0.0.1:0", enableIpc: false))
        defer { hub.close() }
        let gets = Counter(), reads = Counter()
        let app = try await startKv(hub, gets: gets, reads: reads)
        defer { app.stop() }

        let first = try await hub.callTool("kv.get")
        XCTAssertNil(first.error)
        XCTAssertNil(first.cachedAgeMs)
        let second = try await hub.callTool("kv.get")
        XCTAssertNotNil(second.cachedAgeMs, "\(second)")
        XCTAssertEqual(second.dataJSON, first.dataJSON, "命中返回原结果")
        XCTAssertFalse(second.woke)
        XCTAssertEqual(gets.value, 1, "命中不转发给 App")

        let fresh = try await hub.callTool("kv.get", cacheBypass: true)
        XCTAssertNil(fresh.cachedAgeMs)
        XCTAssertEqual(gets.value, 2, "绕过照常调用")
        let again = try await hub.callTool("kv.get")
        XCTAssertEqual(again.dataJSON, fresh.dataJSON, "绕过的新结果覆盖缓存")

        for _ in 0..<2 { _ = try await hub.readResource("app-mcp://kv/snapshot") }
        XCTAssertEqual(reads.value, 1, "声明了 cache 的资源第二次读取命中")

        let cache = try XCTUnwrap(try hub.status().cache)
        XCTAssertEqual([cache.entries, cache.hits, cache.misses], [2, 3, 2], "\(cache)")
        XCTAssertGreaterThan(cache.bytes, 0)
    }

    func testZeroMaxEntriesDisablesCache() async throws {
        let limits = CacheLimitOverrides(maxEntries: 0, maxBytes: nil, maxEntryBytes: nil)
        let hub = try Hub(config: HubConfig(listen: "127.0.0.1:0", enableIpc: false, resultCache: limits))
        defer { hub.close() }
        let gets = Counter(), reads = Counter()
        let app = try await startKv(hub, gets: gets, reads: reads)
        defer { app.stop() }
        for _ in 0..<2 {
            let out = try await hub.callTool("kv.get")
            XCTAssertNil(out.cachedAgeMs)
        }
        XCTAssertEqual(gets.value, 2)
        let cache = try hub.status().cache
        XCTAssertTrue(cache == nil || (cache?.entries == 0 && cache?.hits == 0), "\(String(describing: cache))")
    }
}
