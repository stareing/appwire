@testable import AppMcp
import Foundation
import XCTest

final class LifecycleUnitTests: XCTestCase {
    private func client(_ lifecycle: LifecyclePolicy? = nil) throws -> AppMcpClient {
        try AppMcpClient(config: AppMcpConfig(
            appId: "swift-unit", appName: "单元测试", hostURL: "ws://127.0.0.1:9",
            lifecycle: lifecycle, connectTimeout: 0.5
        ))
    }

    func testPolicyDefaultsMatchSpec() {
        let p = LifecyclePolicy()
        XCTAssertEqual(p.mode, .persistent)
        XCTAssertEqual(p.idleTimeoutMs, 60_000)
        XCTAssertEqual(p.hiddenIdleTimeoutMs, 15_000)
        XCTAssertEqual(p.graceMs, 10_000)
        XCTAssertEqual(p.residency, .keep)
        XCTAssertNil(p.wake)
        let ios = LifecyclePolicy.iOSDefault()
        XCTAssertEqual(ios.mode, .idle)
        XCTAssertEqual(ios.hiddenIdleTimeoutMs, 0)
        #if os(Linux) || os(macOS)
        XCTAssertEqual(LifecyclePolicy.platformDefault, .persistent)
        #endif
    }

    func testPolicyMapsToFfiRecord() {
        let wake = WakeDescriptor.urlScheme("shop", background: true)
        let ffi = LifecyclePolicy(mode: .onDemand, graceMs: 1, residency: .exitWhenIdle, wake: wake).ffi
        XCTAssertEqual(ffi.mode, .onDemand)
        XCTAssertEqual(ffi.graceMs, 1)
        XCTAssertEqual(ffi.residency, .exitWhenIdle)
        XCTAssertEqual(ffi.wake, WakeDescriptor(kind: .uri, target: "shop", background: true))
    }

    func testUrlSchemeDescriptorPlatformDefault() {
        let d = WakeDescriptor.urlScheme("shop")
        XCTAssertEqual(d.kind, .uri)
        XCTAssertEqual(d.target, "shop")
        #if os(macOS)
        XCTAssertTrue(d.background)
        #else
        XCTAssertFalse(d.background)
        #endif
    }

    func testPhaseMapping() {
        XCTAssertEqual(phaseAction(.active, mode: .idle), PhaseAction(visibility: .visible, focused: true, wake: true))
        XCTAssertEqual(phaseAction(.active, mode: .persistent).wake, false)
        XCTAssertEqual(phaseAction(.inactive, mode: .idle), PhaseAction(visibility: .visible, focused: false, wake: false))
        XCTAssertEqual(phaseAction(.background, mode: .onDemand), PhaseAction(visibility: .hidden, focused: false, wake: false))
    }

    func testWakeTokenParsing() {
        XCTAssertEqual(AppMcpClient.wakeToken(in: "app-mcp-wake:abc"), "abc")
        XCTAssertEqual(AppMcpClient.wakeToken(in: "shop://app-mcp/wake?token=t1"), "t1")
        XCTAssertNil(AppMcpClient.wakeToken(in: "shop://cart"))
    }

    func testClientLifecycleApi() throws {
        let c = try client(LifecyclePolicy(mode: .onDemand))
        XCTAssertEqual(c.lifecycle.mode, .onDemand)
        XCTAssertFalse(c.handleWake(url: URL(string: "shop://cart")!))
        XCTAssertFalse(c.handleWake("--verbose"))
        let before = c.toolsHash
        XCTAssertEqual(before.count, 16)
        try c.backgroundTool("t", description: "d") { (_: NoArguments, _) in 1 }
        XCTAssertNotEqual(c.toolsHash, before)
        let hold = c.hold()
        hold.release()
        hold.release()
        _ = c.hold() // deinit 自动释放
        c.setPhase(.background)
        c.stop()
    }

    func testDefaultLifecycleIsPlatformDefault() throws {
        XCTAssertEqual(try client().lifecycle, .platformDefault)
    }

    func testContextHoldWithoutCallThrows() {
        let ctx = ToolContext(callId: "c", toolName: "t", argumentsJSON: "{}")
        XCTAssertThrowsError(try ctx.hold())
    }

    func testToolCallErrorDetails() throws {
        let e = ToolCallError(ErrorKind.invalidInput, "库存不足", details: .object(["available": .number(2)]))
        XCTAssertEqual(try encodeJSON(e.details), #"{"available":2}"#)
        XCTAssertNil(ToolCallError(ErrorKind.handlerError, "x").details)
    }
}
