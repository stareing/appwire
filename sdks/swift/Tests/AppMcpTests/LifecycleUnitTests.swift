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
        XCTAssertEqual(p.hostAbsentRetries, 3)
        XCTAssertFalse(p.legacyTimers)
        XCTAssertEqual(p.mergeWindowMs, 2_000)
        XCTAssertFalse(p.sleepOnBackground)
    }

    func testPlatformDefaults() {
        let wake = WakeDescriptor.urlScheme("shop")
        let ios = LifecyclePolicy.iOSDefault(wake: wake)
        XCTAssertEqual(ios.mode, .onDemand)
        XCTAssertTrue(ios.sleepOnBackground)
        XCTAssertEqual(ios.hiddenIdleTimeoutMs, 0)
        XCTAssertEqual(ios.residency, .keep)
        XCTAssertEqual(ios.wake, wake)
        XCTAssertEqual(ios.mergeWindowMs, 2_000)
        #if os(Linux) || os(macOS)
        XCTAssertEqual(LifecyclePolicy.platformDefault, .persistent)
        XCTAssertEqual(LifecyclePolicy.platformDefault(wake: nil), .persistent)
        let desktop = LifecyclePolicy.platformDefault(wake: wake)
        XCTAssertEqual(desktop, LifecyclePolicy(mode: .idle, wake: wake))
        XCTAssertFalse(desktop.sleepOnBackground)
        #else
        XCTAssertEqual(LifecyclePolicy.platformDefault, .iOSDefault())
        XCTAssertEqual(LifecyclePolicy.platformDefault(wake: wake), ios)
        #endif
    }

    func testExplicitLifecycleWins() throws {
        let own = LifecyclePolicy(mode: .idle, idleTimeoutMs: 5, sleepOnBackground: false)
        XCTAssertEqual(try client(own).lifecycle, own)
        XCTAssertEqual(try client(own).lifecycle.ffi.sleepOnBackground, false)
    }

    func testPowerSwitchesMapToFfiRecord() {
        let d = LifecyclePolicy().ffi
        XCTAssertEqual(d.hostAbsentRetries, 3)
        XCTAssertFalse(d.legacyTimers)
        XCTAssertEqual(d.mergeWindowMs, 2_000)
        XCTAssertFalse(d.sleepOnBackground)
        let p = LifecyclePolicy(hostAbsentRetries: 0, legacyTimers: true, mergeWindowMs: 0, sleepOnBackground: true).ffi
        XCTAssertEqual(p.hostAbsentRetries, 0) // 0 = 一直重连，与 uniffi 编码相同
        XCTAssertTrue(p.legacyTimers)
        XCTAssertEqual(p.mergeWindowMs, 0)
        XCTAssertTrue(p.sleepOnBackground)
    }

    func testHeartbeatMapsToFfiConfig() throws {
        let base = AppMcpConfig(appId: "swift-unit", appName: "心跳")
        XCTAssertEqual(base.heartbeat, .auto)
        XCTAssertEqual(base.ffi(lifecycle: .persistent).heartbeat, .auto)
        for mode in [HeartbeatMode.auto, .always, .off] {
            var c = base
            c.heartbeat = mode
            XCTAssertEqual(c.ffi(lifecycle: .persistent).heartbeat, mode)
        }
        var off = base
        off.hostURL = "ws://127.0.0.1:9"
        off.heartbeat = .off
        _ = try AppMcpClient(config: off)
    }

    func testMaxQueuedCallsMapsToFfiConfig() throws {
        var c = AppMcpConfig(appId: "swift-unit", appName: "排队", hostURL: "ws://127.0.0.1:9")
        XCTAssertNil(c.ffi(lifecycle: .persistent).maxQueuedCalls, "为 nil 时交给核心缺省（64）")
        c.maxQueuedCalls = 0
        XCTAssertEqual(c.ffi(lifecycle: .persistent).maxQueuedCalls, 0)
        c.maxQueuedCalls = 8
        XCTAssertEqual(c.ffi(lifecycle: .persistent).maxQueuedCalls, 8)
        _ = try AppMcpClient(config: c)
    }

    func testRegisterNameMapsToFfiConfig() throws {
        let base = AppMcpConfig(appId: "swift-unit", appName: "按名", hostURL: "ws://127.0.0.1:9")
        XCTAssertFalse(base.ffi(lifecycle: .persistent).registerName)
        XCTAssertNil(base.ffi(lifecycle: .persistent).nameInstance)
        var named = base
        named.registerName = true
        named.nameInstance = "w2"
        let ffi = named.ffi(lifecycle: .persistent)
        XCTAssertTrue(ffi.registerName)
        XCTAssertEqual(ffi.nameInstance, "w2")
        _ = try AppMcpClient(config: named)
        for bad in ["default", "W2", "2w"] {
            var c = base
            c.nameInstance = bad
            XCTAssertThrowsError(try AppMcpClient(config: c), bad) { error in
                guard case AppMcpError.InvalidConfig = error else { return XCTFail("\(bad)：\(error)") }
            }
        }
    }

    func testRealtimeResourceChangesToolsHash() throws {
        func hash(_ register: (AppMcpClient) throws -> Void) throws -> String {
            let c = try client()
            try register(c)
            defer { c.stop() }
            return c.toolsHash
        }
        let plain = try hash { try $0.resource("order", description: "订单") { 1 } }
        let explicitFalse = try hash { try $0.resource("order", description: "订单", realtime: false) { 1 } }
        let realtime = try hash { try $0.resource("order", description: "订单", realtime: true) { 1 } }
        XCTAssertEqual(plain, explicitFalse)
        XCTAssertNotEqual(plain, realtime)
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
