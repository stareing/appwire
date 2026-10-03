import AppMcpHub
import Foundation
import XCTest

/// 对象锁（spec/hub-api.md 3.6「对象锁」）：HubConfig.maxLocks、apps.lock / apps.unlock、HubStatus.locks、LOCKED。
final class LocksTests: XCTestCase {
    private let shop = #"""
        {"manifestVersion":1,"appId":"shop","name":"商城",
         "tools":[{"name":"cart.add","description":"加购","inputSchema":{"type":"object"}}]}
        """#

    private func start(maxLocks: UInt32? = nil) throws -> Hub {
        try Hub(config: HubConfig(enableListen: false, enableIpc: false, manifestsJson: [shop], maxLocks: maxLocks))
    }

    private func json(_ text: String?) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: Data(try XCTUnwrap(text).utf8)) as? [String: Any])
    }

    /// 缺省列出 apps.lock / apps.unlock；会话 s1 加锁后 s2 加同一把锁 → LOCKED（holder = "api"）；status().locks 列出。
    func testLockConflictAndStatus() async throws {
        XCTAssertNil(HubConfig().maxLocks)
        let hub = try start()
        defer { hub.close() }
        let names = Set(hub.tools().map(\.name))
        XCTAssertTrue(names.isSuperset(of: ["apps.lock", "apps.unlock"]), "\(names)")

        let ok = try await hub.callTool("apps.lock", argumentsJSON: #"{"appId":"shop","ttlMs":30000}"#, session: "s1")
        XCTAssertNil(ok.error)
        let denied = try await hub.callTool("apps.lock", argumentsJSON: #"{"appId":"shop"}"#, session: "s2")
        XCTAssertEqual(denied.error?.kind, "LOCKED")
        XCTAssertEqual(try json(denied.error?.detailsJson)["holder"] as? String, "api")

        let locks = try XCTUnwrap(try hub.status().locks)
        XCTAssertEqual(locks.count, 1)
        let lock: LockStatus = locks[0]
        XCTAssertEqual(lock.appId, "shop")
        XCTAssertNil(lock.key)
        XCTAssertEqual(lock.caller, "api:s1")
        XCTAssertEqual(lock.holder, "api")
        XCTAssertTrue((1...30000).contains(lock.expiresInMs), "\(lock)")

        let released = try await hub.callTool("apps.unlock", argumentsJSON: #"{"appId":"shop"}"#, session: "s1")
        XCTAssertEqual(try json(released.dataJSON)["released"] as? Bool, true)
        XCTAssertEqual(try hub.status().locks, [])
    }

    /// maxLocks = 0 关闭对象锁：不列出，调用为 TOOL_NOT_FOUND。
    func testMaxLocksZeroDisables() async throws {
        let hub = try start(maxLocks: 0)
        defer { hub.close() }
        let names = Set(hub.tools().map(\.name))
        XCTAssertTrue(names.isDisjoint(with: ["apps.lock", "apps.unlock"]), "\(names)")
        var kind: String?
        do {
            kind = try await hub.callTool("apps.lock", argumentsJSON: #"{"appId":"shop"}"#, session: "s1").error?.kind
        } catch let HubError.Tool(k, _, _) {
            kind = k
        }
        XCTAssertEqual(kind, "TOOL_NOT_FOUND")
    }
}
