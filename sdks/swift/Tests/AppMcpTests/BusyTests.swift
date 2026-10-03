@testable import AppMcp
import XCTest

/// 用户正在操作（spec/protocol.md 5.3）：开关、策略与引用计数作用域；不需要 Host。拒绝 / 排队行为见一致性用例 call-busy-*。
final class BusyTests: XCTestCase {
    private func makeClient(_ policy: BusyPolicy? = nil) throws -> AppMcpClient {
        try AppMcpClient(config: AppMcpConfig(appId: "swift-unit", appName: "忙碌", hostURL: "ws://127.0.0.1:9", busyPolicy: policy))
    }

    func testSwitchReachesCore() throws {
        let client = try makeClient()
        XCTAssertFalse(client.isBusy)
        client.setBusy(true)
        XCTAssertTrue(client.isBusy)
        client.setBusy(false)
        XCTAssertFalse(client.isBusy)
        client.setBusyPolicy(.queue)
        client.setBusyPolicy(.reject)
    }

    func testBusyPolicyMapsToFfiConfig() throws {
        var c = AppMcpConfig(appId: "swift-unit", appName: "忙碌", hostURL: "ws://127.0.0.1:9")
        XCTAssertNil(c.ffi(lifecycle: .persistent).busyPolicy, "为 nil 时交给核心缺省（reject）")
        c.busyPolicy = .queue
        XCTAssertEqual(c.ffi(lifecycle: .persistent).busyPolicy, .queue)
        _ = try makeClient(.queue)
    }

    func testNestedScopesAreReferenceCounted() throws {
        let client = try makeClient()
        client.withBusy {
            XCTAssertTrue(client.isBusy)
            client.withBusy { XCTAssertTrue(client.isBusy) }
            XCTAssertTrue(client.isBusy, "内层结束不得提前结束外层")
        }
        XCTAssertFalse(client.isBusy)
        struct Oops: Error {}
        XCTAssertThrowsError(try client.withBusy { throw Oops() })
        XCTAssertFalse(client.isBusy, "抛错退出也要归还计数")
        let hold = client.beginBusy()
        hold.release()
        hold.release()
        XCTAssertTrue(hold.isReleased)
        client.withBusy { XCTAssertTrue(client.isBusy, "重复 release 不得多归还计数") }
        XCTAssertFalse(client.isBusy)
    }

    func testHoldReleasedOnDeinit() throws {
        let client = try makeClient()
        do {
            let hold = client.beginBusy()
            XCTAssertTrue(client.isBusy)
            _ = hold
        }
        XCTAssertFalse(client.isBusy)
    }

    func testScopeCombinesWithSwitch() throws {
        let client = try makeClient()
        client.withBusy {
            client.setBusy(false)
            XCTAssertTrue(client.isBusy, "setBusy(false) 不结束进行中的作用域")
        }
        XCTAssertFalse(client.isBusy)
        client.setBusy(true)
        client.withBusy {}
        XCTAssertTrue(client.isBusy, "作用域结束不清除显式开关")
        client.setBusy(false)
        XCTAssertFalse(client.isBusy)
    }

    func testAsyncScope() async throws {
        let client = try makeClient()
        let seen = try await client.withBusy { () async throws -> Bool in
            await Task.yield()
            return client.isBusy
        }
        XCTAssertTrue(seen)
        XCTAssertFalse(client.isBusy)
    }

    func testScopesAcrossThreads() throws {
        let client = try makeClient()
        let entered = DispatchGroup()
        let done = DispatchGroup()
        let release = DispatchSemaphore(value: 0)
        for _ in 0..<2 {
            entered.enter()
            done.enter()
            Thread.detachNewThread {
                client.withBusy {
                    entered.leave()
                    _ = release.wait(timeout: .now() + 5)
                }
                done.leave()
            }
        }
        XCTAssertEqual(entered.wait(timeout: .now() + 5), .success)
        XCTAssertTrue(client.isBusy)
        release.signal()
        release.signal()
        XCTAssertEqual(done.wait(timeout: .now() + 5), .success)
        XCTAssertFalse(client.isBusy)
    }
}
